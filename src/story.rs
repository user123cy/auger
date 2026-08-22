use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::cli::StoryArgs;

#[derive(Serialize, Deserialize)]
struct StoryReport {
    url: String,
    narrative: Vec<StoryEvent>,
    summary: StorySummary,
}

#[derive(Serialize, Deserialize)]
struct StoryEvent {
    time: String,
    event: String,
    severity: String,
    detail: String,
}

#[derive(Serialize, Deserialize)]
struct StorySummary {
    title: String,
    conclusion: String,
    key_findings: Vec<String>,
    recommendation: String,
}

pub async fn run(args: &StoryArgs, json: bool) -> anyhow::Result<()> {
    // Load the saved report
    let text = std::fs::read_to_string(&args.report)
        .map_err(|e| anyhow::anyhow!("failed to read '{}': {}", args.report, e))?;
    let report: crate::stats::Report =
        serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("invalid report JSON: {}", e))?;

    let story = generate_story(&report);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&StoryReport {
                url: report.url.clone(),
                narrative: story.0,
                summary: story.1,
            })?
        );
    } else {
        print_story(&report.url, &story.0, &story.1);
    }

    Ok(())
}

fn generate_story(report: &crate::stats::Report) -> (Vec<StoryEvent>, StorySummary) {
    let mut events = Vec::new();
    let mut findings = Vec::new();
    let stats = report.stats();

    // Opening scene
    events.push(StoryEvent {
        time: "0:00".into(),
        event: "test begins".into(),
        severity: "info".into(),
        detail: format!(
            " {} workers started hammering {} with concurrent requests.",
            report.concurrency, report.url
        ),
    });

    // Analyze the data
    let total = report.requests;
    let errors = report.errors;
    let error_rate = if total > 0 {
        errors as f64 / total as f64 * 100.0
    } else {
        0.0
    };

    // Phase 1: Initial behavior
    let _initial_phase_ms = report.elapsed_ms / 4;
    let initial_requests: u64 = (total as f64 * 0.25) as u64;

    events.push(StoryEvent {
        time: "0:01".into(),
        event: "initial response".into(),
        severity: "info".into(),
        detail: format!(
            " Server is responding. First batch of ~{} requests processed. Average latency: {:.0}ms.",
            initial_requests, stats.mean_ms
        ),
    });

    // Phase 2: Steady state analysis
    if stats.p50 < 50.0 {
        events.push(StoryEvent {
            time: "0:05".into(),
            event: "performance is excellent".into(),
            severity: "success".into(),
            detail: format!(
                " Median response time is only {:.0}ms — the server handles the load轻松.",
                stats.p50
            ),
        });
        findings.push(format!("Excellent median latency: {:.0}ms", stats.p50));
    } else if stats.p50 < 200.0 {
        events.push(StoryEvent {
            time: "0:05".into(),
            event: "performance is acceptable".into(),
            severity: "info".into(),
            detail: format!(
                " Median response time is {:.0}ms — within normal range for a production server.",
                stats.p50
            ),
        });
    } else {
        events.push(StoryEvent {
            time: "0:05".into(),
            event: "server is struggling".into(),
            severity: "warning".into(),
            detail: format!(
                " Median response time is {:.0}ms — the server is under significant load. {} workers may be too many.",
                stats.p50, report.concurrency
            ),
        });
        findings.push(format!("High median latency: {:.0}ms", stats.p50));
    }

    // Phase 3: Tail latency analysis
    let tail_ratio = if stats.p50 > 0.0 {
        stats.p99 / stats.p50
    } else {
        0.0
    };

    if tail_ratio > 5.0 {
        events.push(StoryEvent {
            time: "0:10".into(),
            event: "tail latency spike detected".into(),
            severity: "warning".into(),
            detail: format!(
                " The slowest 1% of requests (p99: {:.0}ms) are {:.0}x slower than the median ({:.0}ms). \
                 This suggests intermittent slowdowns, possibly from garbage collection, cold connections, or queue buildup.",
                stats.p99, tail_ratio, stats.p50
            ),
        });
        findings.push(format!(
            "High tail latency ratio: p99 is {:.1}x p50",
            tail_ratio
        ));
    }

    // Phase 4: Error analysis
    if errors > 0 {
        let error_time = format!(
            "{:02}:{:02}",
            (report.elapsed_ms as f64 * 0.7 / 1000.0) as u64 / 60,
            (report.elapsed_ms as f64 * 0.7 / 1000.0) as u64 % 60
        );

        events.push(StoryEvent {
            time: error_time.clone(),
            event: "errors begin to appear".into(),
            severity: "error".into(),
            detail: format!(
                " {} out of {} requests failed ({:.1}%). \
                 Breakdown: {} timeouts, {} connection errors, {} TLS errors, {} status errors.",
                errors,
                total,
                error_rate,
                report.errors_timeout,
                report.errors_connect,
                report.errors_tls,
                report.errors_status
            ),
        });

        if report.errors_timeout > 0 {
            findings.push(format!(
                "{} timeout errors — server may be overloaded or have slow backends",
                report.errors_timeout
            ));
        }
        if report.errors_connect > 0 {
            findings.push(format!(
                "{} connection errors — server may have hit connection limits",
                report.errors_connect
            ));
        }
        if report.errors_status > 0 {
            findings.push(format!(
                "{} status code errors — server returning errors under load",
                report.errors_status
            ));
        }

        // First error message
        if let Some(first_err) = report.first_errors.first() {
            events.push(StoryEvent {
                time: error_time,
                event: "first error details".into(),
                severity: "error".into(),
                detail: format!(" The first error was: \"{}\"", first_err),
            });
        }
    }

    // Phase 5: Status code distribution story
    if !report.statuses.is_empty() {
        let dominant = report
            .statuses
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(code, count)| (*code, *count))
            .unwrap_or((0, 0));

        let dominant_pct = if total > 0 {
            dominant.1 as f64 / (total as f64 - errors as f64).max(1.0) * 100.0
        } else {
            0.0
        };

        if dominant.0 >= 200 && dominant.0 < 300 && dominant_pct > 95.0 {
            events.push(StoryEvent {
                time: "0:15".into(),
                event: "healthy response distribution".into(),
                severity: "success".into(),
                detail: format!(
                    " {:.1}% of responses are {} — the server maintains healthy status codes throughout the test.",
                    dominant_pct, dominant.0
                ),
            });
            findings.push(format!("Consistent {} responses ({:.1}%)", dominant.0, dominant_pct));
        } else if dominant.0 >= 500 {
            events.push(StoryEvent {
                time: "0:15".into(),
                event: "server returning errors".into(),
                severity: "error".into(),
                detail: format!(
                    " Most responses are {} ({:.1}%) — the server is failing under load.",
                    dominant.0, dominant_pct
                ),
            });
            findings.push(format!("Dominant {} responses ({:.1}%)", dominant.0, dominant_pct));
        }
    }

    // Phase 6: Throughput story
    let rps = stats.rps;
    if rps > 1000.0 {
        events.push(StoryEvent {
            time: "0:20".into(),
            event: "high throughput achieved".into(),
            severity: "success".into(),
            detail: format!(
                " The server sustains {:.0} requests per second — that's {:.0}K req/min.",
                rps, rps * 60.0 / 1000.0
            ),
        });
    }

    // Phase 7: Slowest requests
    if !report.slowest_ms.is_empty() {
        let slowest = report.slowest_ms.first().copied().unwrap_or(0.0);
        events.push(StoryEvent {
            time: "0:25".into(),
            event: "slowest request identified".into(),
            severity: "info".into(),
            detail: format!(
                " The single slowest request took {:.0}ms — that's {:.0}x the median. \
                 This outlier could be a cold cache hit, a slow database query, or GC pause.",
                slowest,
                if stats.p50 > 0.0 {
                    slowest / stats.p50
                } else {
                    0.0
                }
            ),
        });
    }

    // Phase 8: Conclusion
    events.push(StoryEvent {
        time: format!(
            "{:<02}:{:<02}",
            report.elapsed_ms / 60000,
            (report.elapsed_ms % 60000) / 1000
        ),
        event: "test complete".into(),
        severity: "info".into(),
        detail: format!(
            " {} total requests processed. {} bytes ({:.1} MB) downloaded.",
            total,
            report.bytes,
            report.bytes as f64 / (1024.0 * 1024.0)
        ),
    });

    // Build summary
    let title = if errors == 0 && stats.p50 < 200.0 {
        "✅ The server handled the load like a champion".into()
    } else if errors == 0 && stats.p50 < 500.0 {
        "⚠️ The server survived, but showed signs of strain".into()
    } else if errors > 0 && error_rate < 5.0 {
        "⚠️ The server mostly held up, with a few hiccups".into()
    } else if error_rate < 20.0 {
        "🔴 The server buckled under the pressure".into()
    } else {
        "💥 The server collapsed under the load".into()
    };

    let conclusion = if errors == 0 && stats.p50 < 100.0 {
        format!(
            "The server at {} is rock solid. It handled {} concurrent workers for {:.0}s \
             with zero errors and a median response time of {:.0}ms. The p99 of {:.0}ms \
             means even the slowest users had a good experience.",
            report.url, report.concurrency, report.elapsed_ms as f64 / 1000.0, stats.p50, stats.p99
        )
    } else if errors == 0 {
        format!(
            "The server at {} completed the test without errors, but latency was noticeable \
             at {:.0}ms median. Under this load ({:.0} req/s), the server is functional \
             but could benefit from optimization.",
            report.url, stats.p50, stats.rps
        )
    } else {
        format!(
            "The server at {} encountered {} errors ({:.1}%) during the {:.0}s test. \
             This suggests it needs more capacity, better error handling, or investigation \
             into the failing components.",
            report.url, errors, error_rate, report.elapsed_ms as f64 / 1000.0
        )
    };

    let recommendation = if errors == 0 && stats.p50 < 100.0 {
        "Consider increasing load to find the breaking point — the server has headroom.".into()
    } else if errors == 0 && stats.p50 < 500.0 {
        "The server works but consider: adding caching, optimizing database queries, or scaling horizontally.".into()
    } else if report.errors_timeout > report.errors / 2 {
        "Focus on timeout causes: slow backend queries, missing indexes, or connection pool exhaustion.".into()
    } else if report.errors_connect > report.errors / 2 {
        "The server is hitting connection limits. Consider: increasing max_connections, adding connection pooling, or load balancing.".into()
    } else {
        "Investigate error logs for the time window of this test. Check for resource exhaustion, memory leaks, or configuration issues.".into()
    };

    let summary = StorySummary {
        title,
        conclusion,
        key_findings: findings,
        recommendation,
    };

    (events, summary)
}

fn print_story(url: &str, events: &[StoryEvent], summary: &StorySummary) {
    println!();
    println!("  {} {}", "auger story".bold().cyan(), url);
    println!();
    println!("  {}", "─".repeat(60).dimmed());
    println!();

    for event in events {
        let time_str = format!("[{}]", event.time).dimmed().to_string();
        let severity_icon = match event.severity.as_str() {
            "success" => "✅".to_string(),
            "warning" => "⚠️ ".to_string(),
            "error" => "❌".to_string(),
            _ => "ℹ️ ".to_string(),
        };

        let event_name = match event.severity.as_str() {
            "success" => event.event.green().bold().to_string(),
            "warning" => event.event.yellow().bold().to_string(),
            "error" => event.event.red().bold().to_string(),
            _ => event.event.bold().to_string(),
        };

        println!("  {} {} {}", time_str, severity_icon, event_name);

        // Word-wrap the detail text
        let words: Vec<&str> = event.detail.split_whitespace().collect();
        let mut line = String::from("        ");
        for word in words {
            if line.len() + word.len() > 76 {
                println!("  {}", line.dimmed());
                line = format!("        {}", word);
            } else {
                if !line.ends_with(' ') && !line.ends_with('(') {
                    line.push(' ');
                }
                line.push_str(word);
            }
        }
        if !line.trim().is_empty() {
            println!("  {}", line.dimmed());
        }
        println!();
    }

    println!("  {}", "─".repeat(60).dimmed());
    println!();
    println!("  {}", summary.title.bold());
    println!();

    // Word-wrap conclusion
    let words: Vec<&str> = summary.conclusion.split_whitespace().collect();
    let mut line = String::from("  ");
    for word in words {
        if line.len() + word.len() > 76 {
            println!("{}", line);
            line = format!("  {}", word);
        } else {
            if !line.ends_with(' ') && !line.ends_with('(') {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    if !line.trim().is_empty() {
        println!("{}", line);
    }
    println!();

    if !summary.key_findings.is_empty() {
        println!("  {}", "Key findings:".bold().yellow());
        for finding in &summary.key_findings {
            println!("    • {}", finding);
        }
        println!();
    }

    println!(
        "  {} {}",
        "Recommendation:".bold().green(),
        summary.recommendation
    );
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_generates_events() {
        let report = crate::stats::Report {
            url: "http://test.com".into(),
            concurrency: 10,
            elapsed_ms: 5000,
            requests: 1000,
            warmup_discarded: 0,
            errors: 0,
            bytes: 500000,
            latencies_ms: vec![10.0, 20.0, 30.0, 40.0, 50.0],
            errors_timeout: 0,
            errors_connect: 0,
            errors_tls: 0,
            errors_other: 0,
            errors_status: 0,
            statuses: {
                let mut m = std::collections::BTreeMap::new();
                m.insert(200, 1000);
                m
            },
            ttfb_ms: vec![5.0, 10.0, 15.0],
            slowest_ms: vec![50.0],
            first_errors: vec![],
        };
        let (events, summary) = generate_story(&report);
        assert!(!events.is_empty());
        assert!(!summary.title.is_empty());
        assert!(!summary.conclusion.is_empty());
    }
}
