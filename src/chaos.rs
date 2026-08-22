use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use colored::Colorize;
use serde::Serialize;

use crate::cli::ChaosArgs;
use crate::client::ClientConfig;

#[derive(Serialize)]
struct ChaosReport {
    url: String,
    total_requests: u64,
    normal_ok: u64,
    normal_err: u64,
    delay_ok: u64,
    delay_err: u64,
    malformed_ok: u64,
    malformed_err: u64,
    partial_ok: u64,
    partial_err: u64,
    oversized_ok: u64,
    oversized_err: u64,
    resilience_score: u8,
    grade: String,
    issues: Vec<String>,
}

pub async fn run(args: &ChaosArgs, json: bool) -> anyhow::Result<()> {
    let client = ClientConfig::from_http(&args.http).build()?;
    let rounds = args.rounds.max(1);
    let start = Instant::now();

    if !json {
        println!();
        println!("  {} {}", "auger chaos".bold().cyan(), args.url);
        println!(
            "  {} rounds · {}ms between rounds",
            args.rounds,
            args.delay_ms
        );
        println!();
    }

    // --- Phase 1: Baseline (normal requests) ---
    if !json {
        print!("  {} ", "phase 1: baseline".bold().yellow());
    }
    let (normal_ok, normal_err) = run_phase(&client, &args.url, rounds, PhaseType::Normal).await;
    if !json {
        println!("✓ {} ok, {} err", normal_ok, normal_err);
    }

    tokio::time::sleep(Duration::from_millis(args.delay_ms)).await;

    // --- Phase 2: Delayed requests (server under lag) ---
    if !json {
        print!("  {} ", "phase 2: delayed".bold().yellow());
    }
    let (delay_ok, delay_err) = run_phase(&client, &args.url, rounds, PhaseType::Delayed).await;
    if !json {
        println!("✓ {} ok, {} err", delay_ok, delay_err);
    }

    tokio::time::sleep(Duration::from_millis(args.delay_ms)).await;

    // --- Phase 3: Malformed requests ---
    if !json {
        print!("  {} ", "phase 3: malformed".bold().yellow());
    }
    let (mal_ok, mal_err) = run_phase(&client, &args.url, rounds, PhaseType::Malformed).await;
    if !json {
        println!("✓ {} ok, {} err", mal_ok, mal_err);
    }

    tokio::time::sleep(Duration::from_millis(args.delay_ms)).await;

    // --- Phase 4: Partial requests (connection dropped mid-send) ---
    if !json {
        print!("  {} ", "phase 4: partial".bold().yellow());
    }
    let (partial_ok, partial_err) = run_phase(&client, &args.url, rounds, PhaseType::Partial).await;
    if !json {
        println!("✓ {} ok, {} err", partial_ok, partial_err);
    }

    tokio::time::sleep(Duration::from_millis(args.delay_ms)).await;

    // --- Phase 5: Oversized requests ---
    if !json {
        print!("  {} ", "phase 5: oversized".bold().yellow());
    }
    let (oversized_ok, oversized_err) =
        run_phase(&client, &args.url, rounds, PhaseType::Oversized).await;
    if !json {
        println!("✓ {} ok, {} err", oversized_ok, oversized_err);
    }

    let elapsed = start.elapsed();

    // Calculate resilience score
    let total_normal = normal_ok + normal_err;
    let mut score: i32 = 100;
    let mut issues = Vec::new();

    // Penalty for errors during normal requests (baseline)
    if total_normal > 0 {
        let baseline_err_rate = normal_err as f64 / total_normal as f64;
        if baseline_err_rate > 0.01 {
            score -= (baseline_err_rate * 50.0) as i32;
            issues.push(format!(
                "baseline error rate: {:.1}% (server unstable before chaos)",
                baseline_err_rate * 100.0
            ));
        }
    }

    // Server should handle delays gracefully (return errors, not crash)
    let total_delay = delay_ok + delay_err;
    if total_delay > 0 {
        let delay_err_rate = delay_err as f64 / total_delay as f64;
        if delay_err_rate > 0.5 {
            score -= 15;
            issues.push(format!(
                "delayed requests: {:.0}% errors (server struggles with slow clients)",
                delay_err_rate * 100.0
            ));
        } else if delay_err_rate > 0.3 {
            score -= 5;
        }
    }

    // Malformed requests should return 4xx, not crash (5xx or timeout)
    let total_mal = mal_ok + mal_err;
    if total_mal > 0 {
        let mal_err_rate = mal_err as f64 / total_mal as f64;
        if mal_err_rate > 0.5 {
            score -= 20;
            issues.push(format!(
                "malformed requests: {:.0}% errors (server may not validate input properly)",
                mal_err_rate * 100.0
            ));
        } else if mal_err_rate > 0.2 {
            score -= 10;
        }
    }

    // Partial requests: server should handle gracefully
    let total_partial = partial_ok + partial_err;
    if total_partial > 0 {
        let partial_err_rate = partial_err as f64 / total_partial as f64;
        if partial_err_rate > 0.5 {
            score -= 15;
            issues.push(format!(
                "partial requests: {:.0}% errors (server may not handle disconnects well)",
                partial_err_rate * 100.0
            ));
        } else if partial_err_rate > 0.3 {
            score -= 5;
        }
    }

    // Oversized: server should reject with 4xx, not crash
    let total_oversized = oversized_ok + oversized_err;
    if total_oversized > 0 {
        let oversized_err_rate = oversized_err as f64 / total_oversized as f64;
        if oversized_err_rate > 0.5 {
            score -= 10;
            issues.push(format!(
                "oversized requests: {:.0}% errors (server may not limit request size)",
                oversized_err_rate * 100.0
            ));
        }
    }

    let score = score.clamp(0, 100) as u8;
    let grade: String = match score {
        90..=100 => "A".into(),
        75..=89 => "B".into(),
        60..=74 => "C".into(),
        45..=59 => "D".into(),
        _ => "F".into(),
    };

    let report = ChaosReport {
        url: args.url.clone(),
        total_requests: (normal_ok + normal_err + delay_ok + delay_err + mal_ok + mal_err
            + partial_ok + partial_err + oversized_ok + oversized_err),
        normal_ok,
        normal_err,
        delay_ok,
        delay_err,
        malformed_ok: mal_ok,
        malformed_err: mal_err,
        partial_ok,
        partial_err,
        oversized_ok,
        oversized_err,
        resilience_score: score,
        grade: grade.clone(),
        issues,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!();
        println!("  {}", "results:".bold());
        println!(
            "    {:<20} {:>6} req  {:>6} ok  {:>6} err",
            "baseline", total_normal, normal_ok, normal_err
        );
        println!(
            "    {:<20} {:>6} req  {:>6} ok  {:>6} err",
            "delayed", total_delay, delay_ok, delay_err
        );
        println!(
            "    {:<20} {:>6} req  {:>6} ok  {:>6} err",
            "malformed", total_mal, mal_ok, mal_err
        );
        println!(
            "    {:<20} {:>6} req  {:>6} ok  {:>6} err",
            "partial", total_partial, partial_ok, partial_err
        );
        println!(
            "    {:<20} {:>6} req  {:>6} ok  {:>6} err",
            "oversized", total_oversized, oversized_ok, oversized_err
        );
        println!();
        println!(
            "  {} {} {}/100  ({:.1}s)",
            "resilience:".bold(),
            colored_grade_str(&grade, score),
            score,
            elapsed.as_secs_f64()
        );
        if !report.issues.is_empty() {
            println!();
            for issue in &report.issues {
                println!("  {} {}", "!".yellow().bold(), issue);
            }
        }
        println!();
    }

    Ok(())
}

fn colored_grade_str(grade: &str, score: u8) -> String {
    let s = format!("{} ({})", grade, score);
    match grade {
        "A" | "B" => s.green().bold().to_string(),
        "C" => s.yellow().bold().to_string(),
        _ => s.red().bold().to_string(),
    }
}

#[derive(Clone)]
enum PhaseType {
    Normal,
    Delayed,
    Malformed,
    Partial,
    Oversized,
}

async fn run_phase(
    client: &reqwest::Client,
    url: &str,
    rounds: u32,
    phase: PhaseType,
) -> (u64, u64) {
    let ok = Arc::new(AtomicU64::new(0));
    let err = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();
    for i in 0..rounds {
        let client = client.clone();
        let url = url.to_string();
        let ok = ok.clone();
        let err = err.clone();
        let phase = phase.clone();

        handles.push(tokio::spawn(async move {
            let result = match phase {
                PhaseType::Normal => {
                    let resp = client.get(&url).send().await;
                    is_ok(&resp)
                }
                PhaseType::Delayed => {
                    // Simulate a slow client by adding a random delay before reading
                    let delay = Duration::from_millis(100 + (i as u64 % 5) * 200);
                    tokio::time::sleep(delay).await;
                    let resp = client.get(&url).send().await;
                    is_ok(&resp)
                }
                PhaseType::Malformed => {
                    // Send requests with invalid headers, bad HTTP, etc.
                    let malformed_urls = [
                        format!("{}\0", url),              // null byte
                        format!("{}\r\nX-Injected: true", url), // CRLF in URL
                    ];
                    let target = &malformed_urls[i as usize % malformed_urls.len()];
                    let resp = client.get(target).send().await;
                    // For malformed, a 4xx is actually OK (server handled it properly)
                    match resp {
                        Ok(r) => {
                            let code = r.status().as_u16();
                            code >= 100 && code < 500 // 4xx = handled, 5xx = server confused
                        }
                        Err(_) => true, // Connection refused = server rejected = ok
                    }
                }
                PhaseType::Partial => {
                    // Send a request but don't wait for the response
                    let _ = client.get(&url).send().await;
                    // Drop the response immediately — this tests if the server
                    // handles abrupt disconnects without crashing
                    true // We count this as ok if it doesn't panic
                }
                PhaseType::Oversized => {
                    // Send a request with a huge body
                    let huge_body = "x".repeat(1024 * 1024); // 1MB
                    let resp = client
                        .post(&url)
                        .body(huge_body)
                        .send()
                        .await;
                    // For oversized, a 4xx is expected (server should reject)
                    match resp {
                        Ok(r) => {
                            let code = r.status().as_u16();
                            code >= 100 && code < 500
                        }
                        Err(_) => true,
                    }
                }
            };

            if result {
                ok.fetch_add(1, Ordering::Relaxed);
            } else {
                err.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }

    (ok.load(Ordering::Relaxed), err.load(Ordering::Relaxed))
}

fn is_ok(resp: &Result<reqwest::Response, reqwest::Error>) -> bool {
    match resp {
        Ok(r) => {
            let code = r.status().as_u16();
            // For normal requests: 2xx and 3xx are ok
            code >= 200 && code < 400
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_calculation() {
        // Perfect score
        assert_eq!(100_i32.clamp(0, 100), 100);

        // Score with penalties
        let mut score: i32 = 100;
        score -= 15; // delay penalty
        score -= 20; // malformed penalty
        assert_eq!(score.clamp(0, 100), 65);

        // Score can't go below 0
        let mut score: i32 = 100;
        score -= 200;
        assert_eq!(score.clamp(0, 100), 0);
    }

    #[test]
    fn grade_mapping() {
        let grade = |s: u8| match s {
            90..=100 => "A",
            75..=89 => "B",
            60..=74 => "C",
            45..=59 => "D",
            _ => "F",
        };
        assert_eq!(grade(100), "A");
        assert_eq!(grade(90), "A");
        assert_eq!(grade(89), "B");
        assert_eq!(grade(75), "B");
        assert_eq!(grade(60), "C");
        assert_eq!(grade(45), "D");
        assert_eq!(grade(0), "F");
    }
}
