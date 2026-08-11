use std::io;
use std::time::{Duration, Instant};

use colored::Colorize;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::client::TlsStream;

use crate::cli::PingArgs;
use crate::fmt::ms;
use crate::tls::{self, Endpoint, Scheme};

enum Stream {
    Plain(tokio::net::TcpStream),
    Tls(Box<TlsStream<tokio::net::TcpStream>>),
}

#[derive(Serialize)]
pub struct PingResult {
    pub url: String,
    pub dns_ms: f64,
    pub connect_ms: f64,
    pub tls_ms: Option<f64>,
    pub tls_version: Option<String>,
    pub ttfb_ms: f64,
    pub total_ms: f64,
    pub status: u16,
    pub server: Option<String>,
    pub bytes: u64,
    pub http_version: String,
    pub cert_days_left: Option<i64>,
    pub cert_issuer: Option<String>,
}

pub async fn run(args: &PingArgs, json: bool) -> anyhow::Result<()> {
    let ep = tls::parse_endpoint(&args.url, Scheme::Http)?;
    let timeout = Duration::from_secs(args.http.timeout);
    let ua = args
        .http
        .user_agent
        .clone()
        .unwrap_or_else(|| format!("auger/{}", env!("CARGO_PKG_VERSION")));
    let url = display_url(&ep);

    let count = args.count.max(1);
    let mut attempts = Vec::with_capacity(count as usize);
    for _ in 0..count {
        attempts.push(ping_once(&ep, &url, timeout, &ua, &args.http.headers).await?);
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "url": url,
                "attempts": attempts,
            }))?
        );
        return Ok(());
    }

    if attempts.len() == 1 {
        print_single(&attempts[0]);
    } else {
        print_multi(&url, &attempts);
    }
    Ok(())
}

async fn ping_once(
    ep: &Endpoint,
    url: &str,
    timeout: Duration,
    ua: &str,
    headers: &[String],
) -> anyhow::Result<PingResult> {
    let t0 = Instant::now();
    let addrs = match tokio::time::timeout(timeout, tls::resolve(&ep.host, ep.port)).await {
        Ok(Ok(a)) => a,
        Ok(Err(e)) => return Err(e),
        Err(_) => anyhow::bail!("DNS lookup timed out"),
    };
    let dns_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let addr = addrs
        .first()
        .ok_or_else(|| anyhow::anyhow!("no addresses for '{}'", ep.host))?;

    let t1 = Instant::now();
    let stream = io_timeout(timeout, tokio::net::TcpStream::connect(*addr), "connect").await?;
    let connect_ms = t1.elapsed().as_secs_f64() * 1000.0;

    let mut tls_ms = None;
    let mut tls_version = None;
    let mut cert_days_left = None;
    let mut cert_issuer = None;
    let mut stream = if matches!(ep.scheme, Scheme::Https) {
        let t2 = Instant::now();
        let r = match tokio::time::timeout(timeout, tls::handshake(stream, &ep.host)).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(e),
            Err(_) => anyhow::bail!("TLS handshake timed out"),
        };
        tls_ms = Some(t2.elapsed().as_secs_f64() * 1000.0);
        tls_version = Some(r.info.tls_version);
        cert_days_left = Some(r.info.days_left);
        cert_issuer = Some(r.info.issuer);
        Stream::Tls(Box::new(r.stream))
    } else {
        Stream::Plain(stream)
    };

    let t3 = Instant::now();
    let path = if ep.path.is_empty() { "/" } else { &ep.path };
    let mut req = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nConnection: close\r\n",
        path,
        host_header(ep),
        ua
    );
    for h in headers {
        req.push_str(h);
        req.push_str("\r\n");
    }
    req.push_str("\r\n");
    match &mut stream {
        Stream::Plain(s) => {
            io_timeout(timeout, s.write_all(req.as_bytes()), "write request").await?
        }
        Stream::Tls(s) => io_timeout(timeout, s.write_all(req.as_bytes()), "write request").await?,
    }

    let head = match &mut stream {
        Stream::Plain(s) => io_timeout(timeout, read_head(s), "read response").await?,
        Stream::Tls(s) => io_timeout(timeout, read_head(s), "read response").await?,
    };
    let ttfb_ms = t3.elapsed().as_secs_f64() * 1000.0;

    let (status, http_version, header_map) = parse_response_head(&head)?;
    let content_length = header_map
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<u64>().ok());
    let server = header_map
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("server"))
        .map(|(_, v)| v.clone());

    let mut bytes = 0u64;
    let mut buf = [0u8; 8192];
    macro_rules! read_body {
        ($want:expr) => {{
            match &mut stream {
                Stream::Plain(s) => {
                    io_timeout(timeout, s.read(&mut buf[..$want]), "read body").await?
                }
                Stream::Tls(s) => {
                    io_timeout(timeout, s.read(&mut buf[..$want]), "read body").await?
                }
            }
        }};
    }
    match content_length {
        Some(cl) => {
            let mut remaining = cl;
            while remaining > 0 {
                let want = remaining.min(8192) as usize;
                let n = read_body!(want);
                if n == 0 {
                    break;
                }
                bytes += n as u64;
                remaining -= n as u64;
            }
        }
        None => loop {
            let n = read_body!(8192);
            if n == 0 {
                break;
            }
            bytes += n as u64;
        },
    }
    let total_ms = t3.elapsed().as_secs_f64() * 1000.0;

    Ok(PingResult {
        url: url.to_string(),
        dns_ms,
        connect_ms,
        tls_ms,
        tls_version,
        ttfb_ms,
        total_ms,
        status,
        server,
        bytes,
        http_version,
        cert_days_left,
        cert_issuer,
    })
}

/// Read until the end of the response head (`\r\n\r\n`, tolerating `\n\n`).
async fn read_head<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&chunk[..n]);
        if head_terminated(&out) {
            return Ok(out);
        }
        if out.len() > 64 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "response head too large",
            ));
        }
    }
}

fn head_terminated(b: &[u8]) -> bool {
    let n = b.len();
    n >= 4 && b[n - 4..] == *b"\r\n\r\n" || n >= 2 && b[n - 2..] == *b"\n\n"
}

type ResponseHead = (u16, String, Vec<(String, String)>);

fn parse_response_head(head: &[u8]) -> anyhow::Result<ResponseHead> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.lines();
    let status_line = lines.next().unwrap_or("").trim_end_matches('\r');
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or("").to_string();
    let code = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("malformed status line: '{}'", status_line))?
        .trim();
    let status: u16 = code
        .parse()
        .map_err(|_| anyhow::anyhow!("bad status code '{}'", code))?;
    let mut headers = Vec::new();
    for line in lines {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    Ok((status, version, headers))
}

async fn io_timeout<T>(
    dur: Duration,
    fut: impl std::future::Future<Output = io::Result<T>>,
    what: &str,
) -> anyhow::Result<T> {
    match tokio::time::timeout(dur, fut).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(anyhow::anyhow!("{}: {}", what, describe_io(&e))),
        Err(_) => Err(anyhow::anyhow!(
            "{}: timed out after {}s",
            what,
            dur.as_secs()
        )),
    }
}

fn describe_io(e: &io::Error) -> String {
    use io::ErrorKind::*;
    match e.kind() {
        ConnectionRefused => "connection refused — is the server running?".into(),
        TimedOut => "timed out".into(),
        HostUnreachable => "host unreachable".into(),
        AddrNotAvailable => "address not available".into(),
        _ => e.to_string(),
    }
}

fn host_header(ep: &Endpoint) -> String {
    let host = if ep.host.contains(':') {
        format!("[{}]", ep.host)
    } else {
        ep.host.clone()
    };
    if ep.port == ep.scheme.default_port() {
        host
    } else {
        format!("{}:{}", host, ep.port)
    }
}

fn display_url(ep: &Endpoint) -> String {
    let host = if ep.host.contains(':') {
        format!("[{}]", ep.host)
    } else {
        ep.host.clone()
    };
    let authority = if ep.port == ep.scheme.default_port() {
        host
    } else {
        format!("{}:{}", host, ep.port)
    };
    format!("{}://{}{}", ep.scheme.as_str(), authority, ep.path)
}

fn print_single(r: &PingResult) {
    println!();
    println!("  {} {}", "auger ping".bold().cyan(), r.url);
    println!("  {:<8} {} ms", "DNS", ms(r.dns_ms));
    println!("  {:<8} {} ms", "TCP", ms(r.connect_ms));
    if let Some(t) = r.tls_ms {
        let mut line = format!("  {:<8} {} ms", "TLS", ms(t));
        if let Some(v) = &r.tls_version {
            line.push_str(&format!("  (TLS {})", v));
        }
        println!("{}", line);
    }
    println!("  {:<8} {} ms", "TTFB", ms(r.ttfb_ms));
    println!("  {:<8} {} ms", "Total", ms(r.total_ms));
    println!("  {:<8} {}", "status", r.status);
    if let Some(s) = &r.server {
        println!("  {:<8} {}", "server", s);
    }
    println!("  {:<8} {} bytes", "size", r.bytes);
    println!("  {:<8} {}", "http", r.http_version);
    if let (Some(days), Some(issuer)) = (r.cert_days_left, &r.cert_issuer) {
        println!("  {:<8} expires in {} days · {}", "cert", days, issuer);
    }
    println!();
}

fn print_multi(url: &str, results: &[PingResult]) {
    println!();
    println!("  {} {}", "auger ping".bold().cyan(), url);
    println!(
        "  {:>2} {:>6} {:>6} {:>6} {:>6} {:>6}  {:>3}",
        "#", "DNS", "TCP", "TLS", "TTFB", "total", "st"
    );
    for (i, r) in results.iter().enumerate() {
        let tls = r.tls_ms.map(ms).unwrap_or_else(|| "-".into());
        println!(
            "  {:>2} {:>6} {:>6} {:>6} {:>6} {:>6}  {:>3}",
            i + 1,
            ms(r.dns_ms),
            ms(r.connect_ms),
            tls,
            ms(r.ttfb_ms),
            ms(r.total_ms),
            r.status
        );
    }
    for (label, f) in [
        ("DNS", (|r: &PingResult| r.dns_ms) as fn(&PingResult) -> f64),
        (
            "TCP",
            (|r: &PingResult| r.connect_ms) as fn(&PingResult) -> f64,
        ),
        (
            "TTFB",
            (|r: &PingResult| r.ttfb_ms) as fn(&PingResult) -> f64,
        ),
        (
            "total",
            (|r: &PingResult| r.total_ms) as fn(&PingResult) -> f64,
        ),
    ] {
        let (min, avg, max) = stats_of(results, f);
        println!(
            "  {:<5} min {}  avg {}  max {}",
            label,
            ms(min),
            ms(avg),
            ms(max)
        );
    }
    let tls: Vec<f64> = results.iter().filter_map(|r| r.tls_ms).collect();
    if !tls.is_empty() {
        let min = tls.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = tls.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let avg = tls.iter().sum::<f64>() / tls.len() as f64;
        println!(
            "  {:<5} min {}  avg {}  max {}",
            "TLS",
            ms(min),
            ms(avg),
            ms(max)
        );
    }
    println!();
}

fn stats_of(results: &[PingResult], f: fn(&PingResult) -> f64) -> (f64, f64, f64) {
    let mut vals: Vec<f64> = results.iter().map(f).collect();
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let min = *vals.first().unwrap_or(&0.0);
    let max = *vals.last().unwrap_or(&0.0);
    let avg = vals.iter().sum::<f64>() / vals.len() as f64;
    (min, avg, max)
}

#[cfg(test)]
mod tests {
    use super::{head_terminated, parse_response_head};

    #[test]
    fn parses_status_and_headers() {
        let head = b"HTTP/1.1 200 OK\r\nServer: nginx\r\nContent-Length: 5\r\n\r\n";
        let (status, version, headers) = parse_response_head(head).unwrap();
        assert_eq!(status, 200);
        assert_eq!(version, "HTTP/1.1");
        assert_eq!(headers[0], ("Server".into(), "nginx".into()));
        assert_eq!(headers[1], ("Content-Length".into(), "5".into()));
    }

    #[test]
    fn parses_bare_lf_head() {
        let head = b"HTTP/1.1 301 Moved Permanently\nLocation: /new\n\n";
        let (status, _, headers) = parse_response_head(head).unwrap();
        assert_eq!(status, 301);
        assert_eq!(headers[0], ("Location".into(), "/new".into()));
    }

    #[test]
    fn bad_status_line_errors() {
        assert!(parse_response_head(b"garbage").is_err());
    }

    #[test]
    fn head_termination() {
        assert!(head_terminated(b"HTTP/1.1 200 OK\r\n\r\n"));
        assert!(head_terminated(b"a\nb\n\n"));
        assert!(!head_terminated(b"HTTP/1.1 200 OK"));
    }
}
