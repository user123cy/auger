use std::time::Instant;

use colored::Colorize;
use serde::Serialize;

use crate::cli::CheckArgs;
use crate::client::ClientConfig;

const SECURITY: [(&str, &str, u8, &str); 8] = [
    (
        "strict-transport-security",
        "HSTS",
        20,
        "strict-transport-security: max-age=31536000",
    ),
    (
        "content-security-policy",
        "CSP",
        20,
        "content-security-policy: default-src 'self'",
    ),
    (
        "x-frame-options",
        "clickjacking",
        15,
        "x-frame-options: DENY",
    ),
    (
        "x-content-type-options",
        "mime sniffing",
        10,
        "x-content-type-options: nosniff",
    ),
    (
        "referrer-policy",
        "referrer",
        10,
        "referrer-policy: no-referrer",
    ),
    (
        "permissions-policy",
        "permissions",
        10,
        "permissions-policy: geolocation=()",
    ),
    (
        "cross-origin-opener-policy",
        "COOP",
        10,
        "cross-origin-opener-policy: same-origin",
    ),
    (
        "cross-origin-resource-policy",
        "CORP",
        5,
        "cross-origin-resource-policy: same-origin",
    ),
];

#[derive(Serialize)]
struct CheckResult {
    url: String,
    tls: Option<String>,
    status: u16,
    http_version: &'static str,
    server: String,
    bytes: u64,
    ms: f64,
    redirects: Vec<Redirect>,
    cookies: Vec<CookieInfo>,
    grade: char,
    score: u8,
    headers: Vec<HeaderCheck>,
}

#[derive(Serialize)]
struct CookieInfo {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    same_site: Option<String>,
    secure: bool,
    http_only: bool,
}

#[derive(Serialize)]
struct Redirect {
    status: u16,
    url: String,
    ms: f64,
}

#[derive(Serialize)]
struct HeaderCheck {
    label: &'static str,
    present: bool,
    value: Option<String>,
    /// header line to add when missing
    suggest: Option<&'static str>,
}

pub async fn run(args: &CheckArgs, json: bool) -> anyhow::Result<()> {
    let urls = match &args.file {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("failed to read '{}': {}", path, e))?;
            let list: Vec<String> = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect();
            if list.is_empty() {
                anyhow::bail!("file '{}' has no URLs", path);
            }
            list
        }
        None => vec![args.url.clone()],
    };

    let client = ClientConfig::from_http(&args.http)
        .without_redirects()
        .build()?;
    for url in &urls {
        if json {
            match check_url(&client, url).await {
                Ok(r) => println!("{}", serde_json::to_string_pretty(&r)?),
                Err(e) => println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({"url": url, "error": e}))?
                ),
            }
        } else {
            println!("  {} {}", "auger check".bold().cyan(), url);
            match check_url(&client, url).await {
                Ok(r) => print_result(&r),
                Err(e) => println!("  {} {}", "✗".red(), e),
            }
        }
    }
    Ok(())
}

async fn check_url(client: &reqwest::Client, url: &str) -> Result<CheckResult, String> {
    let tls = if url.starts_with("https://") {
        tls_info(url).await
    } else {
        None
    };

    let mut current = url.to_string();
    let mut cookies: Vec<CookieInfo> = Vec::new();
    let mut redirects: Vec<Redirect> = Vec::new();
    let mut hops = 0usize;
    let mut first: Option<(u16, &'static str, String, u64, f64)> = None;
    let final_headers = loop {
        let t0 = Instant::now();
        let resp = match client.get(&current).send().await {
            Ok(r) => r,
            Err(e) => return Err(e.to_string()),
        };
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        for v in headers.get_all("set-cookie") {
            if let Ok(s) = v.to_str() {
                let info = parse_cookie(s);
                if !cookies.iter().any(|c| c.name == info.name) {
                    cookies.push(info);
                }
            }
        }

        if hops == 0 {
            let size = resp.content_length().unwrap_or(0);
            let server = headers
                .get("server")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("-")
                .to_string();
            first = Some((status, version_str(resp.version()), server, size, ms));
        } else {
            redirects.push(Redirect {
                status,
                url: current.clone(),
                ms,
            });
        }

        if resp.status().is_redirection()
            && let Some(loc) = headers.get("location").and_then(|v| v.to_str().ok())
        {
            current = resolve(&current, loc);
            hops += 1;
            if hops > 10 {
                return Err("too many redirects".into());
            }
            continue;
        }
        break headers;
    };

    let (status, http_version, server, bytes, ms) = first.expect("first hop always captured");
    let (score, headers) = security_rows(&final_headers);
    Ok(CheckResult {
        url: url.to_string(),
        tls,
        status,
        http_version,
        server,
        bytes,
        ms,
        redirects,
        cookies,
        grade: grade(score),
        score,
        headers,
    })
}

fn print_result(r: &CheckResult) {
    if let Some(t) = &r.tls {
        println!("  tls     {}", t);
    }
    println!("  status  {}", status_str(r.status));
    println!("  http    {}", r.http_version);
    println!("  server  {}", r.server);
    println!("  {:.1} ms · {} bytes", r.ms, r.bytes);
    for red in &r.redirects {
        println!(
            "  redirect {}  {}  {:.0}ms",
            status_str(red.status),
            red.url,
            red.ms
        );
    }
    if !r.cookies.is_empty() {
        let list: Vec<String> = r.cookies.iter().map(cookie_str).collect();
        println!("  cookies  {}", list.join(" · "));
    }
    println!();
    println!("  security headers");
    for h in &r.headers {
        match (&h.value, h.suggest) {
            (Some(v), _) => println!("  {} {:<13} {}", "✓".green(), h.label, v),
            (None, Some(s)) => println!("  {} {:<13} missing · {}", "✗".red(), h.label, s),
            (None, None) => println!("  {} {:<13} missing", "✗".red(), h.label),
        }
    }
    println!();
    println!("  grade  {}", colored_grade(r.grade, r.score));
}

fn colored_grade(g: char, score: u8) -> String {
    let s = format!("{g}  ({score}/100)");
    match g {
        'A' | 'B' => s.green().bold().to_string(),
        'C' => s.yellow().bold().to_string(),
        _ => s.red().bold().to_string(),
    }
}

fn security_rows(headers: &reqwest::header::HeaderMap) -> (u8, Vec<HeaderCheck>) {
    let mut score = 0u8;
    let mut rows = Vec::with_capacity(SECURITY.len());
    for (name, label, weight, suggest) in SECURITY {
        match headers.get(name).and_then(|v| v.to_str().ok()) {
            Some(v) => {
                score += weight;
                rows.push(HeaderCheck {
                    label,
                    present: true,
                    value: Some(v.to_string()),
                    suggest: None,
                });
            }
            None => rows.push(HeaderCheck {
                label,
                present: false,
                value: None,
                suggest: Some(suggest),
            }),
        }
    }
    (score, rows)
}

fn grade(score: u8) -> char {
    match score {
        90..=100 => 'A',
        75..=89 => 'B',
        60..=74 => 'C',
        45..=59 => 'D',
        _ => 'F',
    }
}

async fn tls_info(url: &str) -> Option<String> {
    let ep = crate::tls::parse_endpoint(url, crate::tls::Scheme::Https).ok()?;
    let r = crate::tls::connect_tls(&ep.host, ep.port).await.ok()?;
    Some(format!(
        "TLS {} · issuer {} · expires {}",
        r.info.tls_version, r.info.issuer, r.info.not_after
    ))
}

fn parse_cookie(raw: &str) -> CookieInfo {
    let mut parts = raw.split(';');
    let name = parts
        .next()
        .unwrap_or(raw)
        .split('=')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let mut secure = false;
    let mut http_only = false;
    let mut same_site = None;
    for attr in parts {
        let lower = attr.trim().to_lowercase();
        if lower == "secure" {
            secure = true;
        } else if lower == "httponly" {
            http_only = true;
        } else if let Some(v) = lower
            .strip_prefix("samesite=")
            .or_else(|| lower.strip_prefix("same-site="))
        {
            same_site = Some(v.to_string());
        }
    }
    CookieInfo {
        name,
        same_site,
        secure,
        http_only,
    }
}

fn cookie_str(c: &CookieInfo) -> String {
    let ok = c.secure && c.http_only && c.same_site.is_some();
    let flags = if ok {
        "ok".to_string()
    } else {
        let mut missing = Vec::new();
        if !c.secure {
            missing.push("no Secure");
        }
        if !c.http_only {
            missing.push("no HttpOnly");
        }
        if c.same_site.is_none() {
            missing.push("no SameSite");
        }
        missing.join(", ")
    };
    let line = format!("{} ({})", c.name, flags);
    if ok {
        line.green().to_string()
    } else {
        line.red().to_string()
    }
}

fn resolve(base: &str, loc: &str) -> String {
    if loc.starts_with("http://") || loc.starts_with("https://") {
        loc.to_string()
    } else if loc.starts_with('/') {
        let scheme = base.split("://").next().unwrap_or("http");
        let host = base
            .split("://")
            .nth(1)
            .and_then(|s| s.split('/').next())
            .unwrap_or("");
        format!("{}://{}{}", scheme, host, loc)
    } else {
        format!(
            "{}/{}",
            base.trim_end_matches('/'),
            loc.trim_start_matches('/')
        )
    }
}

fn status_str(code: u16) -> String {
    let s = code.to_string();
    match code {
        200..=299 => s.green().bold().to_string(),
        300..=399 => s.cyan().to_string(),
        400..=499 => s.yellow().bold().to_string(),
        _ => s.red().bold().to_string(),
    }
}

fn version_str(v: reqwest::Version) -> &'static str {
    match v {
        reqwest::Version::HTTP_09 => "HTTP/0.9",
        reqwest::Version::HTTP_10 => "HTTP/1.0",
        reqwest::Version::HTTP_11 => "HTTP/1.1",
        reqwest::Version::HTTP_2 => "HTTP/2",
        reqwest::Version::HTTP_3 => "HTTP/3",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::{grade, parse_cookie, resolve, security_rows};

    #[test]
    fn grade_boundaries() {
        assert_eq!(grade(100), 'A');
        assert_eq!(grade(90), 'A');
        assert_eq!(grade(89), 'B');
        assert_eq!(grade(75), 'B');
        assert_eq!(grade(74), 'C');
        assert_eq!(grade(60), 'C');
        assert_eq!(grade(59), 'D');
        assert_eq!(grade(45), 'D');
        assert_eq!(grade(44), 'F');
        assert_eq!(grade(0), 'F');
    }

    #[test]
    fn empty_headers_score_zero() {
        let headers = reqwest::header::HeaderMap::new();
        let (score, rows) = security_rows(&headers);
        assert_eq!(score, 0);
        assert_eq!(rows.len(), 8);
        assert!(rows.iter().all(|r| !r.present));
        assert!(rows.iter().all(|r| r.suggest.is_some()));
    }

    #[test]
    fn present_headers_sum_weights() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "strict-transport-security",
            "max-age=63072000".parse().unwrap(),
        );
        headers.insert(
            "content-security-policy",
            "default-src 'self'".parse().unwrap(),
        );
        headers.insert("x-content-type-options", "nosniff".parse().unwrap());
        let (score, rows) = security_rows(&headers);
        assert_eq!(score, 20 + 20 + 10);
        let present = rows.iter().filter(|r| r.present).count();
        assert_eq!(present, 3);
    }

    #[test]
    fn resolve_absolute() {
        assert_eq!(
            resolve("https://a.com/", "https://other.com/x"),
            "https://other.com/x"
        );
    }

    #[test]
    fn resolve_root_relative() {
        assert_eq!(resolve("https://a.com/path", "/new"), "https://a.com/new");
    }

    #[test]
    fn resolve_path_relative() {
        assert_eq!(
            resolve("https://a.com/dir/", "next"),
            "https://a.com/dir/next"
        );
    }

    #[test]
    fn cookie_parses_flags() {
        let c = parse_cookie("session=abc; Path=/; Secure; HttpOnly; SameSite=Lax");
        assert_eq!(c.name, "session");
        assert!(c.secure);
        assert!(c.http_only);
        assert_eq!(c.same_site.as_deref(), Some("lax"));
    }

    #[test]
    fn cookie_plain() {
        let c = parse_cookie("id=1");
        assert_eq!(c.name, "id");
        assert!(!c.secure);
        assert!(!c.http_only);
        assert_eq!(c.same_site, None);
    }
}
