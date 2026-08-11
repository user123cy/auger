use colored::Colorize;
use serde::Serialize;
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::*;
use x509_parser::signature_algorithm::SignatureAlgorithm;

use crate::tls::{self, Scheme};

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
    let ep = tls::parse_endpoint(target, Scheme::Https)?;
    Ok((ep.host, ep.port))
}

async fn fetch(host: &str, port: u16) -> anyhow::Result<CertInfo> {
    let r = tls::connect_tls(host, port).await?;
    let conn = r.stream.get_ref().1;
    let certs = conn
        .peer_certificates()
        .ok_or_else(|| anyhow::anyhow!("server sent no certificate"))?;
    let der = certs
        .first()
        .ok_or_else(|| anyhow::anyhow!("empty certificate chain"))?;
    let (_, cert) = parse_x509_certificate(der.as_ref())
        .map_err(|_| anyhow::anyhow!("could not parse certificate"))?;

    let key_size = cert
        .public_key()
        .parsed()
        .map(|k| k.key_size())
        .unwrap_or(0);
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

    Ok(CertInfo {
        host: host.to_string(),
        port,
        tls: r.info.tls_version,
        chain: certs.len(),
        subject: r.info.subject,
        issuer: r.info.issuer,
        key,
        sig,
        not_before: tls::ymd(cert.validity().not_before.timestamp()),
        not_after: r.info.not_after,
        days_left: r.info.days_left,
        sans,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_target;

    #[test]
    fn target_default_port() {
        assert_eq!(
            parse_target("example.com").unwrap(),
            ("example.com".into(), 443)
        );
        assert_eq!(
            parse_target("https://example.com/").unwrap(),
            ("example.com".into(), 443)
        );
    }

    #[test]
    fn target_custom_port() {
        assert_eq!(
            parse_target("example.com:8443").unwrap(),
            ("example.com".into(), 8443)
        );
        assert_eq!(
            parse_target("https://example.com:9443/x").unwrap(),
            ("example.com".into(), 9443)
        );
    }

    #[test]
    fn target_ipv6() {
        assert_eq!(parse_target("[::1]:8443").unwrap(), ("::1".into(), 8443));
    }
}
