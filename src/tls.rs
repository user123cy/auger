use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rustls::pki_types::ServerName;
use x509_parser::prelude::*;

pub enum Scheme {
    Http,
    Https,
}

impl Scheme {
    pub fn default_port(&self) -> u16 {
        match self {
            Scheme::Http => 80,
            Scheme::Https => 443,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Scheme::Http => "http",
            Scheme::Https => "https",
        }
    }
}

pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub scheme: Scheme,
    pub path: String,
}

/// Parse a host or URL into an endpoint. Without a `scheme://` prefix the
/// given default is used; the path defaults to `/` and brackets are stripped.
pub fn parse_endpoint(url: &str, default_scheme: Scheme) -> anyhow::Result<Endpoint> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        anyhow::bail!("empty target");
    }
    let (scheme, rest) = match trimmed.split_once("://") {
        Some((s, rest)) => {
            let scheme = match s.to_lowercase().as_str() {
                "http" => Scheme::Http,
                "https" => Scheme::Https,
                _ => anyhow::bail!("unsupported scheme '{}' (use http or https)", s),
            };
            (scheme, rest)
        }
        None => (default_scheme, trimmed),
    };

    let (authority, mut path) = match rest.find(['/', '?', '#']) {
        Some(idx) => (&rest[..idx], rest[idx..].to_string()),
        None => (rest, String::new()),
    };
    if let Some(hash) = path.find('#') {
        path.truncate(hash);
    }
    if path.is_empty() {
        path.push('/');
    } else if path.starts_with('?') {
        path.insert(0, '/');
    }

    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, port) = match rest.split_once(']') {
            Some((h, rest)) => {
                let port = match rest.strip_prefix(':') {
                    Some(p) => p
                        .parse::<u16>()
                        .map_err(|_| anyhow::anyhow!("bad port '{}'", p))?,
                    None => scheme.default_port(),
                };
                (h.to_string(), port)
            }
            None => anyhow::bail!("unterminated '[' in '{}'", trimmed),
        };
        (host, port)
    } else {
        match authority.rsplit_once(':') {
            Some((h, p))
                if !h.is_empty() && !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) =>
            {
                (h.to_string(), p.parse::<u16>()?)
            }
            _ => (authority.to_string(), scheme.default_port()),
        }
    };

    if host.is_empty() {
        anyhow::bail!("no host in '{}'", trimmed);
    }
    Ok(Endpoint {
        host,
        port,
        scheme,
        path,
    })
}

pub struct TlsInfo {
    pub tls_version: String,
    pub issuer: String,
    pub subject: String,
    pub not_after: String,
    pub days_left: i64,
}

pub struct TlsResult {
    pub stream: tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    pub info: TlsInfo,
}

pub async fn resolve(host: &str, port: u16) -> anyhow::Result<Vec<std::net::SocketAddr>> {
    Ok(tokio::net::lookup_host((host, port)).await?.collect())
}

/// Wrap a connected TCP stream in TLS, returning the stream and certificate
/// facts about the peer.
pub async fn handshake(stream: tokio::net::TcpStream, host: &str) -> anyhow::Result<TlsResult> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let name = ServerName::try_from(host.to_string())
        .map_err(|_| anyhow::anyhow!("invalid hostname '{}'", host))?;
    let stream = connector.connect(name, stream).await?;
    let info = cert_info(stream.get_ref().1)?;
    Ok(TlsResult { stream, info })
}

pub async fn connect_tls(host: &str, port: u16) -> anyhow::Result<TlsResult> {
    let stream = tokio::net::TcpStream::connect((host, port)).await?;
    handshake(stream, host).await
}

fn cert_info(conn: &rustls::client::ClientConnection) -> anyhow::Result<TlsInfo> {
    let certs = conn
        .peer_certificates()
        .ok_or_else(|| anyhow::anyhow!("server sent no certificate"))?;
    let der = certs
        .first()
        .ok_or_else(|| anyhow::anyhow!("empty certificate chain"))?;
    let (_, cert) = parse_x509_certificate(der.as_ref())
        .map_err(|_| anyhow::anyhow!("could not parse certificate"))?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let not_after = cert.validity().not_after.timestamp();
    let days_left = (not_after - now) / 86400;

    let tls_version = match conn.protocol_version() {
        Some(rustls::ProtocolVersion::TLSv1_3) => "1.3".to_string(),
        Some(rustls::ProtocolVersion::TLSv1_2) => "1.2".to_string(),
        _ => "older".to_string(),
    };

    Ok(TlsInfo {
        tls_version,
        issuer: common_name(cert.issuer()),
        subject: common_name(cert.subject()),
        not_after: ymd(not_after),
        days_left,
    })
}

fn common_name(name: &X509Name) -> String {
    name.iter_common_name()
        .find_map(|atv| atv.as_str().ok().map(String::from))
        .unwrap_or_else(|| name.to_string())
}

/// Format a unix timestamp as YYYY-MM-DD.
pub fn ymd(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let civil = civil_from_days(days);
    format!("{:04}-{:02}-{:02}", civil.0, civil.1, civil.2)
}

// Howard Hinnant's civil_from_days algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(url: &str) -> Endpoint {
        parse_endpoint(url, Scheme::Http).unwrap()
    }

    fn https(url: &str) -> Endpoint {
        parse_endpoint(url, Scheme::Https).unwrap()
    }

    #[test]
    fn default_port_by_scheme() {
        assert_eq!(http("example.com").port, 80);
        assert_eq!(https("example.com").port, 443);
    }

    #[test]
    fn bare_host() {
        let ep = https("example.com");
        assert_eq!(ep.host, "example.com");
        assert_eq!(ep.path, "/");
        assert!(matches!(ep.scheme, Scheme::Https));
    }

    #[test]
    fn custom_port() {
        let ep = https("example.com:8443/x");
        assert_eq!(ep.host, "example.com");
        assert_eq!(ep.port, 8443);
        assert_eq!(ep.path, "/x");
    }

    #[test]
    fn ipv6_bracketed() {
        let ep = https("[::1]:8080");
        assert_eq!(ep.host, "::1");
        assert_eq!(ep.port, 8080);
    }

    #[test]
    fn path_and_query() {
        let ep = http("example.com/a/b?q=1");
        assert_eq!(ep.path, "/a/b?q=1");
        let ep = http("example.com?x=1");
        assert_eq!(ep.path, "/?x=1");
    }

    #[test]
    fn explicit_scheme_wins() {
        let ep = parse_endpoint("https://example.com", Scheme::Http).unwrap();
        assert!(matches!(ep.scheme, Scheme::Https));
        assert_eq!(ep.port, 443);
    }

    #[test]
    fn bad_scheme_is_error() {
        assert!(parse_endpoint("ftp://example.com", Scheme::Http).is_err());
    }

    #[test]
    fn empty_is_error() {
        assert!(parse_endpoint("", Scheme::Http).is_err());
        assert!(parse_endpoint("   ", Scheme::Http).is_err());
    }

    #[test]
    fn ymd_known_dates() {
        assert_eq!(ymd(0), "1970-01-01");
        assert_eq!(ymd(1609459200), "2021-01-01");
        assert_eq!(ymd(1752710400), "2025-07-17");
    }
}
