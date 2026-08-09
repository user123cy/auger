use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use colored::Colorize;
use rustls::pki_types::ServerName;
use serde::Serialize;
use tokio_rustls::TlsConnector;
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::*;
use x509_parser::signature_algorithm::SignatureAlgorithm;

#[derive(Serialize)]
struct CertInfo {
    host: String,
    port: u16,
    tls: String,
    chain: usize,
    subject: String,
    issuer: String,
    key: String,
    sig: String,
    not_before: String,
    not_after: String,
    days_left: i64,
    sans: Vec<String>,
}

pub async fn run(target: &str, json: bool) -> anyhow::Result<()> {
    let (host, port) = parse_target(target)?;
    let info = fetch(&host, port).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&info)?);
    } else {
        print_info(&info);
    }
    if info.days_left < 0 {
        anyhow::bail!(
            "certificate expired {} day(s) ago on {}",
            -info.days_left,
            info.not_after
        );
    }
    if info.days_left < 30 {
        eprintln!(
            "  {} expires in {} days, renew soon",
            "warning".yellow().bold(),
            info.days_left
        );
    }
    Ok(())
}

fn print_info(i: &CertInfo) {
    let host = if i.port == 443 {
        i.host.clone()
    } else {
        format!("{}:{}", i.host, i.port)
    };
    println!("  {} {}", "auger cert".bold().cyan(), host);
    println!("  subject  {}", i.subject);
    println!("  issuer   {}", i.issuer);
    println!("  tls      {}", i.tls);
    println!("  key      {}", i.key);
    println!("  sig      {}", i.sig);
    println!("  chain    {} certs", i.chain);
    println!("  valid    {} to {}", i.not_before, i.not_after);
    if i.days_left < 0 {
        println!("  expired  {} days ago", -i.days_left);
    } else {
        println!("  expires  in {} days", i.days_left);
    }
    if !i.sans.is_empty() {
        println!("  san      {}", i.sans.join(", "));
    }
    println!();
}

fn parse_target(target: &str) -> anyhow::Result<(String, u16)> {
    let rest = target.split("://").last().unwrap_or(target);
    let host_part = rest.split('/').next().unwrap_or("").trim_end_matches('/');
    let (host, port) = match host_part.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            (h.to_string(), p.parse::<u16>()?)
        }
        _ => (host_part.to_string(), 443),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        anyhow::bail!("no host in '{}'", target);
    }
    Ok((host.to_string(), port))
}

async fn fetch(host: &str, port: u16) -> anyhow::Result<CertInfo> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));
    let stream = tokio::net::TcpStream::connect((host, port)).await?;
    let name = ServerName::try_from(host.to_string())
        .map_err(|_| anyhow::anyhow!("invalid hostname '{}'", host))?;
    let tls = connector.connect(name, stream).await?;

    let conn = tls.get_ref().1;
    let certs = conn
        .peer_certificates()
        .ok_or_else(|| anyhow::anyhow!("server sent no certificate"))?;
    let der = certs
        .first()
        .ok_or_else(|| anyhow::anyhow!("empty certificate chain"))?;
    let (_, cert) = parse_x509_certificate(der.as_ref())
        .map_err(|_| anyhow::anyhow!("could not parse certificate"))?;

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let not_after = cert.validity().not_after.timestamp();
    let days_left = (not_after - now) / 86400;

    let key_size = cert.public_key().parsed().map(|k| k.key_size()).unwrap_or(0);
    let key = if key_size > 0 {
        format!("{} bits", key_size)
    } else {
        "unknown".into()
    };
    let sig = match SignatureAlgorithm::try_from(&cert.signature_algorithm) {
        Ok(SignatureAlgorithm::RSA) | Ok(SignatureAlgorithm::RSASSA_PSS(_)) => "RSA".into(),
        Ok(SignatureAlgorithm::DSA) => "DSA".into(),
        Ok(SignatureAlgorithm::ECDSA) => "ECDSA".into(),
        Ok(SignatureAlgorithm::ED25519) => "Ed25519".into(),
        Ok(SignatureAlgorithm::RSAAES_OAEP(_)) => "RSA-OAEP".into(),
        Err(_) => "unknown".into(),
    };

    let mut sans = Vec::new();
    if let Ok(Some(ext)) = cert.subject_alternative_name() {
        for gn in &ext.value.general_names {
            if let GeneralName::DNSName(d) = gn {
                sans.push((*d).to_string());
            }
        }
    }

    let tls = match conn.protocol_version() {
        Some(rustls::ProtocolVersion::TLSv1_3) => "1.3",
        Some(rustls::ProtocolVersion::TLSv1_2) => "1.2",
        _ => "older",
    }
    .to_string();

    Ok(CertInfo {
        host: host.to_string(),
        port,
        tls,
        chain: certs.len(),
        subject: common_name(cert.subject()),
        issuer: common_name(cert.issuer()),
        key,
        sig,
        not_before: ymd(cert.validity().not_before.timestamp()),
        not_after: ymd(not_after),
        days_left,
        sans,
    })
}

fn common_name(name: &X509Name) -> String {
    name.iter_common_name()
        .find_map(|atv| atv.as_str().ok().map(|s| s.to_string()))
        .unwrap_or_else(|| name.to_string())
}

// civil-from-days, no chrono dep
fn ymd(secs: i64) -> String {
    let z = secs.div_euclid(86400) + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!(
        "{:04}-{:02}-{:02}",
        if m <= 2 { y + 1 } else { y },
        m,
        d
    )
}

#[cfg(test)]
mod tests {
    use super::{parse_target, ymd};

    #[test]
    fn target_default_port() {
        assert_eq!(parse_target("example.com").unwrap(), ("example.com".into(), 443));
        assert_eq!(
            parse_target("https://example.com/").unwrap(),
            ("example.com".into(), 443)
        );
    }

    #[test]
    fn target_custom_port() {
        assert_eq!(parse_target("example.com:8443").unwrap(), ("example.com".into(), 8443));
        assert_eq!(
            parse_target("https://example.com:9443/x").unwrap(),
            ("example.com".into(), 9443)
        );
    }

    #[test]
    fn target_ipv6() {
        assert_eq!(parse_target("[::1]:8443").unwrap(), ("::1".into(), 8443));
    }

    #[test]
    fn ymd_known_dates() {
        assert_eq!(ymd(0), "1970-01-01");
        assert_eq!(ymd(1609459200), "2021-01-01");
        assert_eq!(ymd(1752710400), "2025-07-17");
    }
}
