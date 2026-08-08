use std::collections::BTreeMap;
use std::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
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
}

// reqwest 0.12 has no is_tls(), so walk the error chain looking for a TLS origin.
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

pub async fn run(args: &RunArgs) -> anyhow::Result<Report> {
    let method: reqwest::Method = args.method.parse()?;
    let duration = crate::cli::parse_duration(&args.duration)?;
    let workers = args.concurrency.max(1);
    let body = args.body_file.as_deref().map(std::fs::read).transpose()?;
    let ramp = args.ramp.as_deref().map(crate::cli::parse_duration).transpose()?;
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
    let done_p = done.clone();
    let fail_p = fail.clone();
    let progress = tokio::spawn(async move {
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
    });

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
            Some(r) => Duration::from_micros((r.as_micros() as u128 * i as u128 / workers as u128) as u64),
            None => Duration::ZERO,
        };
        let done = done.clone();
        let fail = fail.clone();
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
                if let Some(limit) = limit {
                    if done.load(Ordering::Relaxed) + fail.load(Ordering::Relaxed) >= limit {
                        break;
                    }
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
                        slowest.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
                        slowest.truncate(5);
                        done.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        errors.add(&e);
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

    let mut latencies = Vec::new();
    let mut ttfb_ms = Vec::new();
    let mut statuses = BTreeMap::new();
    let mut total_timeout = 0u64;
    let mut total_connect = 0u64;
    let mut total_tls = 0u64;
    let mut total_other = 0u64;
    let mut total_bytes = 0u64;
    let mut slowest = Vec::new();
    for h in handles {
        if let Ok(Ok((mut samples, mut t, s, e, bytes, mut top))) = h.await {
            latencies.append(&mut samples);
            ttfb_ms.append(&mut t);
            for (k, v) in s {
                *statuses.entry(k).or_insert(0) += v;
            }
            total_timeout += e.timeout;
            total_connect += e.connect;
            total_tls += e.tls;
            total_other += e.other;
            total_bytes += bytes;
            slowest.append(&mut top);
        }
    }
    slowest.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    slowest.truncate(5);
    progress.abort();
    eprintln!();

    let elapsed_ms = start.elapsed().as_millis() as u64;
    let total_errors = total_timeout + total_connect + total_tls + total_other;
    let requests = latencies.len() as u64 + total_errors;

    Ok(Report {
        url: args.url.clone(),
        concurrency: workers,
        elapsed_ms,
        requests,
        errors: total_errors,
        errors_timeout: total_timeout,
        errors_connect: total_connect,
        errors_tls: total_tls,
        errors_other: total_other,
        statuses,
        bytes: total_bytes,
        latencies_ms: latencies,
        ttfb_ms,
        slowest_ms: slowest,
    })
}
