use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use colored::Colorize;
use serde::Serialize;

use crate::cli::FingerprintArgs;
use crate::client::ClientConfig;

#[derive(Serialize)]
struct FingerprintReport {
    url: String,
    fingerprint: String,
    confidence: u8,
    server_type: String,
    characteristics: Characteristics,
    anomalies: Vec<String>,
    stable: bool,
}

#[derive(Serialize)]
struct Characteristics {
    header_order: Vec<String>,
    timing_pattern: String,
    error_signature: String,
    security_posture: String,
    technology_fingerprint: String,
    unique_markers: Vec<String>,
}

pub async fn run(args: &FingerprintArgs, json: bool) -> anyhow::Result<()> {
    let client = ClientConfig::from_http(&args.http).build()?;

    if !json {
        println!();
        println!("  {} {}", "auger fingerprint".bold().cyan(), args.url);
        println!("  analyzing server behavior...");
        println!();
    }

    let start = Instant::now();

    // Phase 1: Normal request — capture header order and timing
    let (normal_headers, normal_timing, _normal_status) = probe_normal(&client, &args.url).await;

    // Phase 2: Error request — see how server handles bad input
    let (error_headers, error_timing, error_status) = probe_error(&client, &args.url).await;

    // Phase 3: Method probing — different methods reveal different behavior
    let method_responses = probe_methods(&client, &args.url).await;

    // Phase 4: Header injection — detect filtering/proxy behavior
    let injection_response = probe_injection(&client, &args.url).await;

    // Phase 5: Timing consistency — run multiple requests to detect patterns
    let timing_consistency = probe_timing_consistency(&client, &args.url).await;

    // Build characteristics
    let header_order: Vec<String> = normal_headers.keys().map(|k| k.as_str().to_lowercase()).collect();
    let header_fingerprint = header_order.join(",");

    let timing_pattern = classify_timing(normal_timing);
    let error_sig = classify_error(error_status);
    let security_posture = assess_security(&normal_headers);
    let tech_fingerprint = detect_tech_pattern(&normal_headers);
    let unique_markers = find_unique_markers(&normal_headers, &injection_response);

    // Check stability across requests
    let stable = timing_consistency.is_consistent;

    // Build the fingerprint hash
    let fingerprint = build_fingerprint(
        &header_fingerprint,
        &timing_pattern,
        &error_sig,
        &tech_fingerprint,
        &unique_markers,
    );

    // Detect server type
    let server_type = detect_server_type(&normal_headers, &error_headers);

    // Detect anomalies
    let anomalies = detect_anomalies(
        &normal_headers,
        &error_headers,
        normal_timing,
        error_timing,
        &method_responses,
    );

    // Calculate confidence
    let confidence = calculate_confidence(
        &header_fingerprint,
        normal_timing,
        &tech_fingerprint,
        &unique_markers,
    );

    let elapsed = start.elapsed();

    let report = FingerprintReport {
        url: args.url.clone(),
        fingerprint: fingerprint.clone(),
        confidence,
        server_type,
        characteristics: Characteristics {
            header_order: header_order.clone(),
            timing_pattern,
            error_signature: error_sig,
            security_posture,
            technology_fingerprint: tech_fingerprint,
            unique_markers,
        },
        anomalies,
        stable,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("  {} {}", "fingerprint:".bold().cyan(), fingerprint.dimmed());
        println!("  {:<22} {}", "server type:".bold(), report.server_type);
        println!(
            "  {:<22} {}%",
            "confidence:".bold(),
            confidence
        );
        println!(
            "  {:<22} {}",
            "timing pattern:".bold(),
            report.characteristics.timing_pattern
        );
        println!(
            "  {:<22} {}",
            "error signature:".bold(),
            report.characteristics.error_signature
        );
        println!(
            "  {:<22} {}",
            "security posture:".bold(),
            report.characteristics.security_posture
        );
        if !report.characteristics.technology_fingerprint.is_empty() {
            println!(
                "  {:<22} {}",
                "technology:".bold(),
                report.characteristics.technology_fingerprint
            );
        }
        println!(
            "  {:<22} {}",
            "header order:".bold(),
            header_order.join(" → ").dimmed()
        );
        println!(
            "  {:<22} {}",
            "stable timing:".bold(),
            if stable {
                "yes".green().to_string()
            } else {
                "no (varies)".yellow().to_string()
            }
        );

        if !report.characteristics.unique_markers.is_empty() {
            println!();
            println!("  {}", "unique markers:".bold().yellow());
            for marker in &report.characteristics.unique_markers {
                println!("    • {}", marker);
            }
        }

        if !report.anomalies.is_empty() {
            println!();
            println!("  {}", "anomalies:".bold().yellow());
            for anomaly in &report.anomalies {
                println!("    {} {}", "!".yellow(), anomaly);
            }
        }

        println!();
        println!(
            "  analyzed in {:.1}s",
            elapsed.as_secs_f64()
        );
        println!();
    }

    Ok(())
}

async fn probe_normal(
    client: &reqwest::Client,
    url: &str,
) -> (reqwest::header::HeaderMap, f64, u16) {
    let t0 = Instant::now();
    match client.get(url).send().await {
        Ok(resp) => {
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            let headers = resp.headers().clone();
            let status = resp.status().as_u16();
            let _ = resp.bytes().await; // consume body
            (headers, ms, status)
        }
        Err(_) => (reqwest::header::HeaderMap::new(), 0.0, 0),
    }
}

async fn probe_error(
    client: &reqwest::Client,
    url: &str,
) -> (reqwest::header::HeaderMap, f64, u16) {
    // Request a non-existent path to trigger error response
    let error_url = format!("{}/auger_nonexistent_{}", url.trim_end_matches('/'), 12345);
    let t0 = Instant::now();
    match client.get(&error_url).send().await {
        Ok(resp) => {
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            let headers = resp.headers().clone();
            let status = resp.status().as_u16();
            let _ = resp.bytes().await;
            (headers, ms, status)
        }
        Err(_) => (reqwest::header::HeaderMap::new(), 0.0, 0),
    }
}

async fn probe_methods(
    client: &reqwest::Client,
    url: &str,
) -> BTreeMap<String, u16> {
    let methods = ["GET", "HEAD", "OPTIONS", "POST", "PUT", "DELETE", "PATCH"];
    let mut results = BTreeMap::new();

    for method in &methods {
        let req = match *method {
            "GET" => client.get(url),
            "HEAD" => client.head(url),
            "OPTIONS" => client.request(reqwest::Method::OPTIONS, url),
            "POST" => client.post(url),
            "PUT" => client.put(url),
            "DELETE" => client.delete(url),
            "PATCH" => client.patch(url),
            _ => continue,
        };

        if let Ok(resp) = req.send().await {
            let status = resp.status().as_u16();
            results.insert(method.to_string(), status);
            let _ = resp.bytes().await;
        }
    }
    results
}

async fn probe_injection(
    client: &reqwest::Client,
    url: &str,
) -> reqwest::header::HeaderMap {
    // Send requests with suspicious headers to see if the server filters them
    match client
        .get(url)
        .header("X-Forwarded-For", "127.0.0.1")
        .header("X-Real-IP", "127.0.0.1")
        .header("X-Original-URL", "/admin")
        .send()
        .await
    {
        Ok(resp) => {
            let headers = resp.headers().clone();
            let _ = resp.bytes().await;
            headers
        }
        Err(_) => reqwest::header::HeaderMap::new(),
    }
}

struct TimingConsistency {
    is_consistent: bool,
    avg_ms: f64,
    stddev_ms: f64,
}

async fn probe_timing_consistency(
    client: &reqwest::Client,
    url: &str,
) -> TimingConsistency {
    let samples = 10;
    let mut timings = Vec::new();

    for _ in 0..samples {
        let t0 = Instant::now();
        if let Ok(resp) = client.get(url).send().await {
            let _ = resp.bytes().await;
            timings.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    if timings.is_empty() {
        return TimingConsistency {
            is_consistent: false,
            avg_ms: 0.0,
            stddev_ms: 0.0,
        };
    }

    let avg = timings.iter().sum::<f64>() / timings.len() as f64;
    let variance = timings
        .iter()
        .map(|t| (t - avg).powi(2))
        .sum::<f64>()
        / timings.len() as f64;
    let stddev = variance.sqrt();

    // Consider consistent if stddev < 20% of average
    let is_consistent = if avg > 0.0 {
        stddev / avg < 0.20
    } else {
        false
    };

    TimingConsistency {
        is_consistent,
        avg_ms: avg,
        stddev_ms: stddev,
    }
}

fn classify_timing(ms: f64) -> String {
    match ms as u64 {
        0..=50 => "fast (<50ms)".into(),
        51..=200 => "normal (50-200ms)".into(),
        201..=500 => "slow (200-500ms)".into(),
        501..=2000 => "very slow (500ms-2s)".into(),
        _ => "extremely slow (>2s)".into(),
    }
}

fn classify_error(status: u16) -> String {
    match status {
        0 => "no response".into(),
        404 => "proper 404".into(),
        400 => "proper 400 (validates input)".into(),
        301 | 302 => "redirects to valid page".into(),
        403 => "403 forbidden".into(),
        406 => "content negotiation".into(),
        429 => "rate limited".into(),
        500 => "500 internal error (crashes on bad input)".into(),
        502 | 503 | 504 => "5xx (proxy/backend issues)".into(),
        s if s >= 400 && s < 500 => format!("{} (proper client error)", s),
        s if s >= 500 => format!("{} (server error on bad input)", s),
        _ => format!("{} (unusual)", status),
    }
}

fn assess_security(headers: &reqwest::header::HeaderMap) -> String {
    let mut score = 0u8;
    if headers.contains_key("strict-transport-security") {
        score += 2;
    }
    if headers.contains_key("content-security-policy") {
        score += 2;
    }
    if headers.contains_key("x-content-type-options") {
        score += 1;
    }
    if headers.contains_key("x-frame-options") {
        score += 1;
    }
    if headers.contains_key("permissions-policy") {
        score += 1;
    }

    // Check for security anti-patterns
    let has_cors = headers.contains_key("access-control-allow-origin");
    let server_exposed = headers
        .get("server")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.contains('/') && s.len() > 20)
        .unwrap_or(false);

    match score {
        0..=2 => {
            if has_cors && server_exposed {
                "weak (minimal headers, server version exposed, CORS wide open)".into()
            } else if has_cors {
                "weak (minimal headers, CORS enabled)".into()
            } else {
                "weak (minimal security headers)".into()
            }
        }
        3..=4 => "moderate (some security headers present)".into(),
        5..=6 => "strong (most security headers present)".into(),
        7 => "excellent (all security headers present)".into(),
        _ => "moderate".into(),
    }
}

fn detect_tech_pattern(headers: &reqwest::header::HeaderMap) -> String {
    let mut markers = Vec::new();

    if let Some(server) = headers.get("server").and_then(|v| v.to_str().ok()) {
        markers.push(server.to_string());
    }
    if let Some(powered) = headers
        .get("x-powered-by")
        .and_then(|v| v.to_str().ok())
    {
        markers.push(powered.to_string());
    }
    if let Some(generator) = headers
        .get("x-generator")
        .and_then(|v| v.to_str().ok())
    {
        markers.push(generator.to_string());
    }
    if let Some(request_id) = headers.get("x-request-id").and_then(|v| v.to_str().ok()) {
        if request_id.len() > 20 {
            markers.push("UUID request ID".into());
        } else if request_id.chars().all(|c| c.is_ascii_digit()) {
            markers.push("sequential request ID".into());
        }
    }

    markers.join(" | ")
}

fn find_unique_markers(
    normal: &reqwest::header::HeaderMap,
    injection: &reqwest::header::HeaderMap,
) -> Vec<String> {
    let mut markers = Vec::new();

    // Check for custom headers that are unique to this server
    let custom_headers = [
        "x-request-id",
        "x-runtime",
        "x-amzn-requestid",
        "x-amz-cf-id",
        "x-cache",
        "x-varnish",
        "x-debug-token",
        "x-debug-token-link",
        "x-backend",
        "x-upstream",
        "x-powered-by",
        "x-aspnet-version",
        "x-generator",
        "x-drupal-cache",
        "x-pantheon-styx-hostname",
        "x-served-by",
        "x-hw",
    ];

    for header in &custom_headers {
        if let Some(val) = normal.get(*header).and_then(|v| v.to_str().ok()) {
            markers.push(format!("{}: {}", header, val));
        }
    }

    // Check if X-Forwarded-For was reflected (proxy detection)
    if let Some(xff) = injection
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
    {
        if xff.contains("127.0.0.1") {
            markers.push("X-Forwarded-For reflected (proxy detected)".into());
        }
    }

    markers
}

fn build_fingerprint(
    header_fp: &str,
    timing: &str,
    error: &str,
    tech: &str,
    markers: &[String],
) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    header_fp.hash(&mut hasher);
    timing.hash(&mut hasher);
    error.hash(&mut hasher);
    tech.hash(&mut hasher);
    for m in markers {
        m.hash(&mut hasher);
    }

    let hash = hasher.finish();
    format!("auger-{:016x}", hash)
}

fn detect_server_type(
    normal: &reqwest::header::HeaderMap,
    _error: &reqwest::header::HeaderMap,
) -> String {
    // Check error page for technology hints
    let server = normal
        .get("server")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let powered = normal
        .get("x-powered-by")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !server.is_empty() && !powered.is_empty() {
        format!("{} ({})", server, powered)
    } else if !server.is_empty() {
        server.to_string()
    } else if !powered.is_empty() {
        powered.to_string()
    } else {
        "unknown (no identifying headers)".into()
    }
}

fn detect_anomalies(
    _normal: &reqwest::header::HeaderMap,
    error: &reqwest::header::HeaderMap,
    normal_ms: f64,
    error_ms: f64,
    methods: &BTreeMap<String, u16>,
) -> Vec<String> {
    let mut anomalies = Vec::new();

    // Error responses should not be slower than normal (unless logging)
    if error_ms > normal_ms * 3.0 && error_ms > 100.0 {
        anomalies.push(format!(
            "error responses are {:.1}x slower than normal ({:.0}ms vs {:.0}ms) — possible heavy error logging",
            error_ms / normal_ms, error_ms, normal_ms
        ));
    }

    // Check if error page leaks information
    if let Some(server) = error.get("server").and_then(|v| v.to_str().ok()) {
        if server.len() > 20 && server.contains('/') {
            anomalies.push(format!(
                "error responses expose server version: {}",
                server
            ));
        }
    }

    // Check for unusual method support
    let allows_delete = methods.get("DELETE").copied().unwrap_or(0);
    let allows_trace = methods.get("TRACE").copied().unwrap_or(0);
    if allows_delete == 200 || allows_delete == 204 {
        anomalies.push("DELETE method returns 200/204 (may allow resource deletion)".into());
    }
    if allows_trace == 200 {
        anomalies.push("TRACE method enabled (potential XST vulnerability)".into());
    }

    // Check for timing attack potential
    if normal_ms > 0.0 && (error_ms - normal_ms).abs() < 1.0 && normal_ms < 10.0 {
        anomalies.push(
            "constant response time (~0ms difference between normal and error) — may indicate cached/static responses"
                .into(),
        );
    }

    anomalies
}

fn calculate_confidence(
    header_fp: &str,
    timing_ms: f64,
    tech: &str,
    markers: &[String],
) -> u8 {
    let mut confidence: i32 = 0;

    // More headers = more data to fingerprint
    let header_count = header_fp.split(',').count();
    if header_count > 10 {
        confidence += 30;
    } else if header_count > 5 {
        confidence += 20;
    } else if header_count > 2 {
        confidence += 10;
    }

    // Having a server header is very identifying
    if !tech.is_empty() {
        confidence += 30;
    }

    // Unique markers are highly identifying
    confidence += ((markers.len() * 10) as i32).min(30);

    // Having timing data increases confidence
    if timing_ms > 0.0 {
        confidence += 10;
    }

    confidence.clamp(0, 100) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_timing_works() {
        assert!(classify_timing(10.0).contains("fast"));
        assert!(classify_timing(100.0).contains("normal"));
        assert!(classify_timing(300.0).contains("slow"));
        assert!(classify_timing(1500.0).contains("very slow"));
    }

    #[test]
    fn classify_error_works() {
        assert!(classify_error(404).contains("proper 404"));
        assert!(classify_error(500).contains("500"));
        assert!(classify_error(0).contains("no response"));
    }

    #[test]
    fn build_fingerprint_deterministic() {
        let a = build_fingerprint("a,b,c", "fast", "proper 404", "nginx", &[]);
        let b = build_fingerprint("a,b,c", "fast", "proper 404", "nginx", &[]);
        assert_eq!(a, b);
    }

    #[test]
    fn build_fingerprint_changes_with_input() {
        let a = build_fingerprint("a,b,c", "fast", "proper 404", "nginx", &[]);
        let b = build_fingerprint("x,y,z", "slow", "500 error", "apache", &[]);
        assert_ne!(a, b);
    }
}
