use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context;

use crate::cli::RunArgs;
use crate::client::ClientConfig;
use crate::stats::Report;

#[cfg(feature = "tui")]
use crate::tui::TuiState;

#[derive(Default)]
struct ErrCount {
    timeout: u64,
    connect: u64,
    tls: u64,
    status: u64,
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

    fn add_status(&mut self) {
        self.status += 1;
    }

    fn add_all(&mut self, o: &ErrCount) {
        self.timeout += o.timeout;
        self.connect += o.connect;
        self.tls += o.tls;
        self.status += o.status;
        self.other += o.other;
    }

    fn total(&self) -> u64 {
        self.timeout + self.connect + self.tls + self.status + self.other
    }

    fn transport(&self) -> u64 {
        self.total() - self.status
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

// Which status codes count as a success. Each item is either an exact code
// (200) or a class (2xx).
#[derive(Clone)]
struct StatusOk(Vec<StatusSpec>);

#[derive(Clone, Copy)]
enum StatusSpec {
    Exact(u16),
    Class(u16),
}

impl StatusOk {
    fn parse(spec: &str) -> anyhow::Result<Self> {
        let mut out = Vec::new();
        for part in spec.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if part.len() == 3 && part.ends_with("xx") {
                let c = part.as_bytes()[0];
                if c.is_ascii_digit() && c != b'0' {
                    out.push(StatusSpec::Class((c - b'0') as u16 * 100));
                    continue;
                }
            }
            let code: u16 = part
                .parse()
                .with_context(|| format!("bad status code '{part}' in --status-ok"))?;
            if !(100..=599).contains(&code) {
                anyhow::bail!("status code {code} out of range in --status-ok");
            }
            out.push(StatusSpec::Exact(code));
        }
        if out.is_empty() {
            anyhow::bail!("--status-ok needs at least one code or class, e.g. '2xx,3xx'");
        }
        Ok(Self(out))
    }

    fn contains(&self, code: u16) -> bool {
        self.0.iter().any(|s| match s {
            StatusSpec::Exact(c) => *c == code,
            StatusSpec::Class(base) => code >= *base && code < base + 100,
        })
    }
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

struct WorkerParams {
    config: ClientConfig,
    url: String,
    method: reqwest::Method,
    body: Option<Vec<u8>>,
    ramp_delay: Duration,
    deadline: Instant,
    warmup_end: Option<Instant>,
    limit: Option<u64>,
    max_errors: Option<u64>,
    per_worker: Option<Duration>,
    status_ok: StatusOk,
}

struct WorkerShared {
    done: Arc<AtomicU64>,
    fail: Arc<AtomicU64>,
    first_errs: Arc<Mutex<Vec<String>>>,
    pause: Option<Arc<AtomicBool>>,
    report: Option<Arc<tokio::sync::RwLock<Report>>>,
    start: Instant,
    workers: u32,
}

async fn worker(p: WorkerParams, sh: WorkerShared) -> anyhow::Result<WorkerOut> {
    let client = p.config.build()?;
    if !p.ramp_delay.is_zero() {
        tokio::time::sleep(p.ramp_delay).await;
    }
    let mut samples = Vec::new();
    let mut ttfb_ms = Vec::new();
    let mut statuses = BTreeMap::new();
    let mut errors = ErrCount::default();
    let mut bytes = 0u64;
    let mut slowest = Vec::new();
    while Instant::now() < p.deadline {
        if let Some(limit) = p.limit
            && sh.done.load(Ordering::Relaxed) + sh.fail.load(Ordering::Relaxed) >= limit
        {
            break;
        }
        if let Some(max) = p.max_errors
            && sh.fail.load(Ordering::Relaxed) >= max
        {
            break;
        }
        if let Some(pause) = &sh.pause {
            while pause.load(Ordering::Relaxed) && Instant::now() < p.deadline {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        let t0 = Instant::now();
        let mut req = client.request(p.method.clone(), &p.url);
        if let Some(b) = &p.body {
            req = req.body(b.clone());
        }
        let req = req.build()?;
        match client.execute(req).await {
            Ok(resp) => {
                let code = resp.status().as_u16();
                *statuses.entry(code).or_insert(0) += 1;
                let ttfb = t0.elapsed().as_secs_f64() * 1000.0;
                let total = match resp.bytes().await {
                    Ok(b) => {
                        bytes += b.len() as u64;
                        t0.elapsed().as_secs_f64() * 1000.0
                    }
                    Err(_) => ttfb,
                };
                if !p.status_ok.contains(code) {
                    errors.add_status();
                }
                if p.warmup_end.is_none_or(|w| t0 >= w) {
                    ttfb_ms.push(ttfb);
                    samples.push(total);
                    slowest.push(total);
                    slowest.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
                    slowest.truncate(5);
                }
                sh.done.fetch_add(1, Ordering::Relaxed);
                if let Some(report) = &sh.report
                    && sh.done.load(Ordering::Relaxed).is_multiple_of(10)
                {
                    let mut r = report.write().await;
                    r.url = p.url.clone();
                    r.concurrency = sh.workers;
                    r.elapsed_ms = sh.start.elapsed().as_millis() as u64;
                    r.requests = sh.done.load(Ordering::Relaxed) + sh.fail.load(Ordering::Relaxed);
                    r.errors = errors.total();
                    r.errors_timeout = errors.timeout;
                    r.errors_connect = errors.connect;
                    r.errors_tls = errors.tls;
                    r.errors_status = errors.status;
                    r.errors_other = errors.other;
                    r.statuses = statuses.clone();
                    r.bytes = bytes;
                    r.latencies_ms = samples.clone();
                    r.ttfb_ms = ttfb_ms.clone();
                    r.slowest_ms = slowest.clone();
                }
            }
            Err(e) => {
                errors.add(&e);
                if let Ok(mut v) = sh.first_errs.lock()
                    && v.len() < 3
                {
                    v.push(error_detail(&e));
                }
                sh.fail.fetch_add(1, Ordering::Relaxed);
            }
        }
        if let Some(d) = &p.per_worker {
            tokio::time::sleep(*d).await;
        }
    }
    Ok((samples, ttfb_ms, statuses, errors, bytes, slowest))
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
    total_sent: u64,
    first_errors: Vec<String>,
) -> Report {
    let total_errors = c.errors.total();
    let requests = c.latencies.len() as u64 + c.errors.transport();
    Report {
        url,
        concurrency: workers,
        elapsed_ms,
        requests,
        // Requests sent during the warmup window are excluded from the stats;
        // surface the gap so the summary reconciles with the live counter.
        warmup_discarded: total_sent.saturating_sub(requests),
        errors: total_errors,
        errors_timeout: c.errors.timeout,
        errors_connect: c.errors.connect,
        errors_tls: c.errors.tls,
        errors_status: c.errors.status,
        errors_other: c.errors.other,
        statuses: c.statuses,
        bytes: c.bytes,
        latencies_ms: c.latencies,
        ttfb_ms: c.ttfb_ms,
        slowest_ms: c.slowest,
        first_errors,
    }
}

struct RunOpts {
    method: reqwest::Method,
    duration: Duration,
    workers: u32,
    body: Option<Vec<u8>>,
    ramp: Option<Duration>,
    limit: Option<u64>,
    per_worker: Option<Duration>,
    warmup: Option<Duration>,
    max_errors: Option<u64>,
    status_ok: StatusOk,
}

fn opts(args: &RunArgs) -> anyhow::Result<RunOpts> {
    Ok(RunOpts {
        method: args.method.parse()?,
        duration: crate::cli::parse_duration(&args.duration)?,
        workers: args.concurrency.max(1),
        body: match (&args.body_file, &args.body) {
            (_, Some(b)) => Some(b.as_bytes().to_vec()),
            (Some(f), None) => Some(std::fs::read(f)?),
            (None, None) => None,
        },
        ramp: args
            .ramp
            .as_deref()
            .map(crate::cli::parse_duration)
            .transpose()?,
        limit: args.requests,
        per_worker: args.rps.map(|r| {
            Duration::from_micros((1_000_000u64 / r.max(1)) * args.concurrency.max(1) as u64)
        }),
        warmup: args
            .warmup
            .as_deref()
            .map(crate::cli::parse_duration)
            .transpose()?,
        max_errors: args.max_errors,
        status_ok: StatusOk::parse(&args.status_ok)?,
    })
}

fn worker_params(
    url: &str,
    o: &RunOpts,
    args: &RunArgs,
    i: u32,
    deadline: Instant,
    warmup_end: Option<Instant>,
) -> WorkerParams {
    WorkerParams {
        config: ClientConfig::from_http(&args.http)
            .worker(i as usize)
            .with_random_ua(args.random_ua)
            .with_basic(args.basic.clone())
            .with_token(args.token.clone()),
        url: url.to_string(),
        method: o.method.clone(),
        body: o.body.clone(),
        ramp_delay: match o.ramp {
            Some(r) => {
                Duration::from_micros((r.as_micros() * i as u128 / o.workers as u128) as u64)
            }
            None => Duration::ZERO,
        },
        deadline,
        warmup_end,
        limit: o.limit,
        max_errors: o.max_errors,
        per_worker: o.per_worker,
        status_ok: o.status_ok.clone(),
    }
}

pub async fn run(url: String, args: &RunArgs, quiet: bool) -> anyhow::Result<Report> {
    let o = opts(args)?;

    // Fail fast on bad proxy, headers or auth before any traffic is sent.
    ClientConfig::from_http(&args.http)
        .with_basic(args.basic.clone())
        .with_token(args.token.clone())
        .build()?;

    let start = Instant::now();
    let warmup_end = o.warmup.map(|w| start + w);
    let deadline = start + o.warmup.unwrap_or(Duration::ZERO) + o.duration;

    let done = Arc::new(AtomicU64::new(0));
    let fail = Arc::new(AtomicU64::new(0));
    let first_errs = Arc::new(Mutex::new(Vec::new()));
    let progress = progress(&done, &fail, quiet);

    let mut handles = Vec::new();
    for i in 0..o.workers {
        let p = worker_params(&url, &o, args, i, deadline, warmup_end);
        let sh = WorkerShared {
            done: done.clone(),
            fail: fail.clone(),
            first_errs: first_errs.clone(),
            pause: None,
            report: None,
            start,
            workers: o.workers,
        };
        handles.push(tokio::spawn(worker(p, sh)));
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

    let elapsed_ms = elapsed_with_warmup(start, o.warmup);
    let total_sent = done.load(Ordering::Relaxed) + fail.load(Ordering::Relaxed);
    Ok(build_report(
        url,
        o.workers,
        elapsed_ms,
        collected,
        total_sent,
        first_errors,
    ))
}

pub async fn run_many(urls: &[String], args: &RunArgs, quiet: bool) -> anyhow::Result<Vec<Report>> {
    let mut out = Vec::with_capacity(urls.len());
    for url in urls {
        out.push(run(url.clone(), args, quiet).await?);
    }
    Ok(out)
}

fn webhook_line(r: &Report) -> String {
    let s = r.stats();
    format!(
        "{} · {} req · {:.0} req/s · {} errors · p50 {:.0}ms · p99 {:.0}ms",
        r.url,
        crate::fmt::group(r.requests),
        s.rps,
        crate::fmt::group(r.errors),
        s.p50,
        s.p99
    )
}

// Post a one-line summary to a Discord or Slack webhook. Failures are reported
// on stderr and never fail the run itself.
pub async fn post_webhook(url: &str, reports: &[Report]) {
    let result = async {
        if reports.is_empty() {
            return Ok(());
        }
        let msg = match reports.len() {
            1 => format!("⚡ auger: {}", webhook_line(&reports[0])),
            n => {
                let mut lines: Vec<String> = reports.iter().map(webhook_line).collect();
                let winner = crate::stats::winner(reports)
                    .map(|i| format!("\n🥇 winner: {}", reports[i].url))
                    .unwrap_or_default();
                lines.push(format!("({n} URLs compared){winner}"));
                format!("⚡ auger battle:\n{}", lines.join("\n"))
            }
        };
        let payload = if url.contains("discord.com") {
            serde_json::json!({ "content": msg })
        } else {
            serde_json::json!({ "text": msg })
        };
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()?;
        let resp = client.post(url).json(&payload).send().await?;
        if !resp.status().is_success() {
            eprintln!("  webhook: got HTTP {}", resp.status());
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if let Err(e) = result {
        eprintln!("  webhook failed: {e}");
    }
}

#[cfg(feature = "tui")]
pub async fn run_tui(url: String, args: &RunArgs) -> anyhow::Result<Report> {
    let o = opts(args)?;

    // Fail fast on bad proxy, headers or auth before any traffic is sent.
    ClientConfig::from_http(&args.http)
        .with_basic(args.basic.clone())
        .with_token(args.token.clone())
        .build()?;

    // Initialize TUI state
    let tui_state = TuiState::new(url.clone(), o.workers, o.duration);
    let report_arc = tui_state.report();
    let tui_stop = tui_state.stop_flag();
    let tui_pause = tui_state.pause_flag();

    let start = Instant::now();
    let warmup_end = o.warmup.map(|w| start + w);
    let deadline = start + o.warmup.unwrap_or(Duration::ZERO) + o.duration;

    let done = Arc::new(AtomicU64::new(0));
    let fail = Arc::new(AtomicU64::new(0));
    let first_errs = Arc::new(Mutex::new(Vec::new()));

    let mut handles = Vec::new();
    for i in 0..o.workers {
        let p = worker_params(&url, &o, args, i, deadline, warmup_end);
        let sh = WorkerShared {
            done: done.clone(),
            fail: fail.clone(),
            first_errs: first_errs.clone(),
            pause: Some(tui_pause.clone()),
            report: Some(report_arc.clone()),
            start,
            workers: o.workers,
        };
        handles.push(tokio::spawn(worker(p, sh)));
    }

    // Run TUI on its own thread: it is synchronous (draw + input poll) and
    // would otherwise occupy a tokio worker forever without yielding.
    let tui_thread = std::thread::spawn(move || {
        let _ = crate::tui::run_tui(tui_state);
    });

    let collected = collect_data(handles).await;
    let first_errors = match Arc::try_unwrap(first_errs) {
        Ok(m) => m.into_inner().unwrap_or_else(|p| p.into_inner()),
        Err(_) => Vec::new(),
    };

    // Stop the TUI and wait for it to leave raw mode / alternate screen
    tui_stop.store(true, Ordering::Relaxed);
    let _ = tui_thread.join();

    let elapsed_ms = elapsed_with_warmup(start, o.warmup);
    let total_sent = done.load(Ordering::Relaxed) + fail.load(Ordering::Relaxed);
    Ok(build_report(
        url,
        o.workers,
        elapsed_ms,
        collected,
        total_sent,
        first_errors,
    ))
}

fn elapsed_with_warmup(start: Instant, warmup: Option<Duration>) -> u64 {
    let warmup_ms = warmup.map(|w| w.as_millis()).unwrap_or(0);
    (start.elapsed().as_millis() as i64 - warmup_ms as i64).max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_ok_parses_codes_and_classes() {
        let ok = StatusOk::parse("2xx,3xx,401").unwrap();
        assert!(ok.contains(200));
        assert!(ok.contains(301));
        assert!(ok.contains(401));
        assert!(!ok.contains(404));
        assert!(!ok.contains(500));
    }

    #[test]
    fn status_ok_default() {
        let ok = StatusOk::parse("2xx,3xx").unwrap();
        assert!(ok.contains(204));
        assert!(ok.contains(399));
        assert!(!ok.contains(400));
    }

    #[test]
    fn status_ok_single_code() {
        let ok = StatusOk::parse("200").unwrap();
        assert!(ok.contains(200));
        assert!(!ok.contains(201));
    }

    #[test]
    fn status_ok_bad_input_errors() {
        assert!(StatusOk::parse("").is_err());
        assert!(StatusOk::parse("abc").is_err());
        assert!(StatusOk::parse("99").is_err());
        assert!(StatusOk::parse("0xx").is_err());
        assert!(StatusOk::parse("600").is_err());
    }

    #[test]
    fn status_ok_tolerates_empty_parts() {
        assert!(StatusOk::parse("2xx,").is_ok());
        assert!(StatusOk::parse("200, 3xx").unwrap().contains(302));
    }

    #[test]
    fn warmup_discarded_reconciles_total_sent() {
        let mut c = Collected::default();
        c.latencies.push(10.0);
        c.latencies.push(20.0);
        c.errors.connect = 1;
        let r = build_report("http://test".into(), 5, 1000, c, 5, vec![]);
        assert_eq!(r.requests, 3);
        assert_eq!(r.warmup_discarded, 2);
    }

    #[test]
    fn no_warmup_means_no_discard() {
        let mut c = Collected::default();
        c.latencies.push(10.0);
        let r = build_report("http://test".into(), 1, 1000, c, 1, vec![]);
        assert_eq!(r.requests, 1);
        assert_eq!(r.warmup_discarded, 0);
    }
}
