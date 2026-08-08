use std::time::Instant;

use colored::Colorize;

use crate::cli::CheckArgs;
use crate::client::ClientConfig;

pub async fn run(args: &CheckArgs) -> anyhow::Result<()> {
    let client = ClientConfig::from_http(&args.http).build()?;
    let t0 = Instant::now();
    let resp = client.get(&args.url).send().await?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;

    let status = resp.status().as_u16();
    let version = resp.version();
    let headers = resp.headers().clone();
    let size = resp.content_length().unwrap_or(0);
    let server = headers
        .get("server")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");

    println!();
    println!("  {} {}", "auger check".bold().cyan(), args.url);
    println!("  status  {}", status_str(status));
    println!("  http    {}", version_str(version));
    println!("  server  {}", server);
    println!("  {:.1} ms · {} bytes", ms, size);
    println!();
    println!("  security headers");
    let checks = [
        ("strict-transport-security", "HSTS"),
        ("content-security-policy", "CSP"),
        ("x-frame-options", "clickjacking"),
        ("x-content-type-options", "mime sniffing"),
        ("referrer-policy", "referrer"),
        ("permissions-policy", "permissions"),
        ("cross-origin-opener-policy", "COOP"),
        ("cross-origin-resource-policy", "CORP"),
    ];
    for (name, label) in checks {
        match headers.get(name).and_then(|v| v.to_str().ok()) {
            Some(v) => println!("  {} {:<13} {}", "✓".green(), label, v),
            None => println!("  {} {:<13} missing", "✗".red(), label),
        }
    }
    Ok(())
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