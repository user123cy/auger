use std::sync::Arc;
use std::time::Instant;

use colored::Colorize;

use crate::cli::CheckArgs;
use crate::client::ClientConfig;

pub async fn run(args: &CheckArgs) -> anyhow::Result<()> {
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

    let client = ClientConfig::from_http(&args.http).without_redirects().build()?;
    for url in &urls {
        check_url(&client, url).await;
    }
    Ok(())
}

async fn check_url(client: &reqwest::Client, url: &str) {
    println!();
    println!("  {} {}", "auger check".bold().cyan(), url);
    if url.starts_with("https://") {
        if let Some(t) = tls_info(url).await {
            println!("  tls     {}", t);
        }
    }

    let mut current = url.to_string();
    let mut cookies: Vec<String> = Vec::new();
    let mut hops = 0usize;
    let final_headers = loop {
        let t0 = Instant::now();
        let resp = match client.get(&current).send().await {
            Ok(r) => r,
            Err(e) => {
                println!("  {} {}", "✗".red(), e);
                return;
            }
        };
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        for v in headers.get_all("set-cookie") {
            if let Ok(s) = v.to_str() {
                let name = s.split(';').next().unwrap_or(s).trim().to_string();
                if !cookies.contains(&name) {
                    cookies.push(name);
                }
            }
        }

        if hops == 0 {
            let size = resp.content_length().unwrap_or(0);
            let server = headers
                .get("server")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("-");
            println!("  status  {}", status_str(status));
            println!("  http    {}", version_str(resp.version()));
            println!("  server  {}", server);
            println!("  {:.1} ms · {} bytes", ms, size);
        } else {
            println!("  redirect {}  {}  {}  {:.0}ms", hops, status_str(status), current, ms);
        }

        if resp.status().is_redirection() {
            match headers.get("location").and_then(|v| v.to_str().ok()) {
                Some(loc) => {
                    current = resolve(&current, loc);
                    hops += 1;
                    if hops > 10 {
                        println!("  {} too many redirects", "✗".red());
                        return;
                    }
                    continue;
                }
                None => {}
            }
        }
        break headers;
    };

    if !cookies.is_empty() {
        println!("  cookies  {}", cookies.join(", "));
    }
    print_security(&final_headers);
}

fn print_security(headers: &reqwest::header::HeaderMap) {
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
}

async fn tls_info(url: &str) -> Option<String> {
    use rustls::pki_types::ServerName;
    use tokio_rustls::TlsConnector;

    let rest = url.split("://").nth(1)?;
    let host = rest.split('/').next()?;
    let (host, port) = match host.split_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().ok()?),
        None => (host.to_string(), 443),
    };

    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().map(|ta| ta.to_owned()));
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));
    let stream = tokio::net::TcpStream::connect((host.as_str(), port)).await.ok()?;
    let name = ServerName::try_from(host).ok()?;
    let tls = connector.connect(name, stream).await.ok()?;

    let conn = tls.get_ref().1;
    let der = conn.peer_certificates()?.into_iter().next()?;
    let (_, cert) = x509_parser::parse_x509_certificate(der.as_ref()).ok()?;
    let ver = match conn.protocol_version()? {
        rustls::ProtocolVersion::TLSv1_3 => "1.3",
        rustls::ProtocolVersion::TLSv1_2 => "1.2",
        _ => "older",
    };
    let not_after = cert.validity().not_after.to_rfc2822().ok()?;
    Some(format!("TLS {} · issuer {} · expires {}", ver, cert.issuer(), not_after))
}

fn resolve(base: &str, loc: &str) -> String {
    if loc.starts_with("http://") || loc.starts_with("https://") {
        loc.to_string()
    } else if loc.starts_with('/') {
        let scheme = base.split("://").next().unwrap_or("http");
        let host = base.split("://").nth(1).and_then(|s| s.split('/').next()).unwrap_or("");
        format!("{}://{}{}", scheme, host, loc)
    } else {
        format!("{}/{}", base.trim_end_matches('/'), loc.trim_start_matches('/'))
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
