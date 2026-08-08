use std::time::Instant;

use crate::cli::RunArgs;
use crate::client::ClientConfig;
use crate::stats::Report;

pub async fn run(args: &RunArgs) -> anyhow::Result<Report> {
    let method: reqwest::Method = args.method.parse()?;
    let duration = crate::cli::parse_duration(&args.duration)?;

    let start = Instant::now();
    let deadline = start + duration;
    let workers = args.concurrency.max(1);

    // Fail fast on bad proxy, headers or auth before any traffic is sent.
    ClientConfig::from_http(&args.http)
        .with_basic(args.basic.clone())
        .with_token(args.token.clone())
        .build()?;

    // Each worker keeps its own samples so we never fight over a shared mutex.
    let mut handles = Vec::new();
    for i in 0..workers {
        let url = args.url.clone();
        let method = method.clone();
        let config = ClientConfig::from_http(&args.http)
            .worker(i as usize)
            .with_random_ua(args.random_ua)
            .with_basic(args.basic.clone())
            .with_token(args.token.clone());
        handles.push(tokio::spawn(async move {
            let client = config.build()?;
            let mut samples = Vec::new();
            let mut errors = 0u64;
            let mut bytes = 0u64;
            while Instant::now() < deadline {
                let t0 = Instant::now();
                match client.request(method.clone(), &url).send().await {
                    Ok(resp) => {
                        bytes += resp.content_length().unwrap_or(0);
                        samples.push(t0.elapsed().as_secs_f64() * 1000.0);
                    }
                    Err(_) => errors += 1,
                }
            }
            Ok::<_, anyhow::Error>((samples, errors, bytes))
        }));
    }

    let mut latencies = Vec::new();
    let mut total_errors = 0u64;
    let mut total_bytes = 0u64;
    for h in handles {
        if let Ok(Ok((mut samples, errors, bytes))) = h.await {
            latencies.append(&mut samples);
            total_errors += errors;
            total_bytes += bytes;
        }
    }
    let elapsed_ms = start.elapsed().as_millis() as u64;
    let requests = latencies.len() as u64 + total_errors;

    Ok(Report {
        url: args.url.clone(),
        concurrency: workers,
        elapsed_ms,
        requests,
        errors: total_errors,
        bytes: total_bytes,
        latencies_ms: latencies,
    })
}
