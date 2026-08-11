use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::cli::RunArgs;
use crate::client::ClientConfig;
use crate::stats::Report;

#[derive(Default)]
struct ErrCount {
    timeout: u64,
    connect: u64,
    tls: u64,
    other: u64,
}

impl ErrCount {
    fn add(&mut self, e: &reqwest::Error) {
        if e.is_timeout() {
            self.timeout += 1;
        } else if is_tls(e) {
            self.tls += 1;
        } else if e.is_connect() {
            self.connect += 1;
        } else {
            self.other += 1;
        }
    }

    fn add_all(&mut self, o: &ErrCount) {
        self.timeout += o.timeout;
        self.connect += o.connect;
        self.tls += o.tls;
        self.other += o.other;
    }
}

type WorkerOut = (
    Vec<f64>,
    Vec<f64>,
    BTreeMap<u16, u64>,
    ErrCount,
    u64,
    Vec<f64>,
);

#[derive(Default)]
struct Collected {
    latencies: Vec<f64>,
    ttfb_ms: Vec<f64>,
    statuses: BTreeMap<u16, u64>,
    errors: ErrCount,
    bytes: u64,
    slowest: Vec<f64>,
}

// reqwest 0.12 has no is_tls(), so walk the error chain looking for a TLS origin.
// reqwest wraps the real cause, so walk to the deepest message.
fn error_detail(e: &reqwest::Error) -> String {
    let mut msg = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        msg = s.to_string();
        src = s.source();
    }
    msg
}

fn is_tls(e: &reqwest::Error) -> bool {
    if !e.is_connect() {
        return false;
    }
    let mut src: Option<&dyn std::error::Error> = e.source();
    while let Some(s) = src {
        let msg = s.to_string().to_lowercase();
        if msg.contains("tls") || msg.contains("ssl") || msg.contains("certificate") {
            return true;
        }
        src = s.source();
    }
    false
}

fn progress(
    done: &Arc<AtomicU64>,
    fail: &Arc<AtomicU64>,
    quiet: bool,
) -> Option<tokio::task::JoinHandle<()>> {
    if quiet {
        return None;
    }
    let done_p = done.clone();
    let fail_p = fail.clone();
    Some(tokio::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_secs(1));
        iv.tick().await;
        loop {
            iv.tick().await;
            eprint!(
                "\r\x1b[2K  {} req · {} errors",
                done_p.load(Ordering::Relaxed),
                fail_p.load(Ordering::Relaxed)
            );
        }
    }))
}

async fn collect_data(
    handles: Vec<tokio::task::JoinHandle<anyhow::Result<WorkerOut>>>,
) -> Collected {
    let mut out = Collected::default();
    for h in handles {
        if let Ok(Ok((mut samples, mut ttfb, statuses, errors, bytes, mut slowest))) = h.await {
            out.latencies.append(&mut samples);
            out.ttfb_ms.append(&mut ttfb);
            for (k, v) in statuses {
                *out.statuses.entry(k).or_insert(0) += v;
            }
            out.errors.add_all(&errors);
            out.bytes += bytes;
            out.slowest.append(&mut slowest);
        }
    }
    out.slowest
        .sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    out.slowest.truncate(5);
    out
}

fn build_report(
    url: String,
    workers: u32,
    elapsed_ms: u64,
    c: Collected,
    first_errors: Vec<String>,
) -> Report {
    let total_errors = c.errors.timeout + c.errors.connect + c.errors.tls + c.errors.other;
    Report {
        url,
        concurrency: workers,
        elapsed_ms,
        requests: c.latencies.len() as u64 + total_errors,
        errors: total_errors,
        errors_timeout: c.errors.timeout,
        errors_connect: c.errors.connect,
        errors_tls: c.errors.tls,
        errors_other: c.errors.other,
        statuses: c.statuses,
        bytes: c.bytes,
        latencies_ms: c.latencies,
        ttfb_ms: c.ttfb_ms,
        slowest_ms: c.slowest,
        first_errors,
    }
}

pub async fn run(args: &RunArgs, quiet: bool) -> anyhow::Result<Report> {
    let method: reqwest::Method = args.method.parse()?;
    let duration = crate::cli::parse_duration(&args.duration)?;
    let workers = args.concurrency.max(1);
    let body = match (&args.body_file, &args.body) {
        (_, Some(b)) => Some(b.as_bytes().to_vec()),
        (Some(f), None) => Some(std::fs::read(f)?),
        (None, None) => None,
    };
    let ramp = args
        .ramp
        .as_deref()
        .map(crate::cli::parse_duration)
        .transpose()?;
    let limit = args.requests;
    let per_worker = args
        .rps
        .map(|r| Duration::from_micros((1_000_000u64 / r.max(1)) * workers as u64));

    // Fail fast on bad proxy, headers or auth before any traffic is sent.
    ClientConfig::from_http(&args.http)
        .with_basic(args.basic.clone())
        .with_token(args.token.clone())
        .build()?;

    let start = Instant::now();
    let deadline = start + duration;

    let done = Arc::new(AtomicU64::new(0));
    let fail = Arc::new(AtomicU64::new(0));
    let first_errs = Arc::new(Mutex::new(Vec::new()));
    let progress = progress(&done, &fail, quiet);

    let mut handles = Vec::new();
    for i in 0..workers {
        let url = args.url.clone();
        let method = method.clone();
        let body = body.clone();
        let config = ClientConfig::from_http(&args.http)
            .worker(i as usize)
            .with_random_ua(args.random_ua)
            .with_basic(args.basic.clone())
            .with_token(args.token.clone());
        let ramp_delay = match ramp {
            Some(r) => Duration::from_micros((r.as_micros() * i as u128 / workers as u128) as u64),
            None => Duration::ZERO,
        };
        let done = done.clone();
        let fail = fail.clone();
        let first_errs = first_errs.clone();
        handles.push(tokio::spawn(async move {
            let client = config.build()?;
            if !ramp_delay.is_zero() {
                tokio::time::sleep(ramp_delay).await;
            }
            let mut samples = Vec::new();
            let mut ttfb_ms = Vec::new();
            let mut statuses = BTreeMap::new();
            let mut errors = ErrCount::default();
            let mut bytes = 0u64;
            let mut slowest = Vec::new();
            while Instant::now() < deadline {
                if let Some(limit) = limit
                    && done.load(Ordering::Relaxed) + fail.load(Ordering::Relaxed) >= limit
                {
                    break;
                }
                let t0 = Instant::now();
                let mut req = client.request(method.clone(), &url);
                if let Some(b) = &body {
                    req = req.body(b.clone());
                }
                let req = req.build()?;
                match client.execute(req).await {
                    Ok(resp) => {
                        *statuses.entry(resp.status().as_u16()).or_insert(0) += 1;
                        let ttfb = t0.elapsed().as_secs_f64() * 1000.0;
                        let total = match resp.bytes().await {
                            Ok(b) => {
                                bytes += b.len() as u64;
                                t0.elapsed().as_secs_f64() * 1000.0
                            }
                            Err(_) => ttfb,
                        };
                        ttfb_ms.push(ttfb);
                        samples.push(total);
                        slowest.push(total);
                        slowest
                            .sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
                        slowest.truncate(5);
                        done.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        errors.add(&e);
                        if let Ok(mut v) = first_errs.lock()
                            && v.len() < 3
                        {
                            v.push(error_detail(&e));
                        }
                        fail.fetch_add(1, Ordering::Relaxed);
                    }
                }
                if let Some(d) = &per_worker {
                    tokio::time::sleep(*d).await;
                }
            }
            Ok::<_, anyhow::Error>((samples, ttfb_ms, statuses, errors, bytes, slowest))
        }));
    }

    let collected = collect_data(handles).await;
    let first_errors = match Arc::try_unwrap(first_errs) {
        Ok(m) => m.into_inner().unwrap_or_else(|p| p.into_inner()),
        Err(_) => Vec::new(),
    };
    if let Some(p) = progress {
        p.abort();
    }
    if !quiet {
        eprintln!();
    }

    let elapsed_ms = start.elapsed().as_millis() as u64;
    Ok(build_report(
        args.url.clone(),
        workers,
        elapsed_ms,
        collected,
        first_errors,
    ))
}
