use std::sync::Arc;

use colored::Colorize;
use serde::Serialize;

use crate::cli::DnsArgs;

#[derive(Serialize)]
struct DnsReport {
    domain: String,
    records: Vec<DnsRecord>,
    subdomains: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct DnsRecord {
    #[serde(rename = "type")]
    record_type: String,
    name: String,
    value: String,
    ttl: u32,
}

pub async fn run(args: &DnsArgs, json: bool) -> anyhow::Result<()> {
    let domain = extract_domain(&args.domain);

    if !json {
        println!();
        println!("  {} {}", "auger dns".bold().cyan(), domain);
        println!();
    }

    let mut records = Vec::new();
    let mut warnings = Vec::new();

    // Use DNS-over-HTTPS to resolve records
    let doh_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let record_types = if args.all {
        vec!["A", "AAAA", "MX", "NS", "TXT", "CNAME", "SOA", "SRV", "CAA", "DMARC"]
    } else {
        args.types
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
    };

    for &rtype in &record_types {
        match fetch_doh(&doh_client, &domain, rtype).await {
            Ok(mut recs) => {
                records.append(&mut recs);
            }
            Err(e) => {
                if json {
                    // skip errors in json mode
                } else if args.verbose {
                    println!("  {} {}: {}", "·".dimmed(), rtype, e);
                }
            }
        }
    }

    // Check for common security issues
    check_security(&records, &mut warnings);

    // Try subdomain enumeration if requested
    let mut subdomains = Vec::new();
    if args.subdomains || args.wordlist.is_some() {
        if let Some(path) = &args.wordlist {
            subdomains = enumerate_subdomains(&doh_client, &domain, path, args.concurrency).await;
        } else if args.subdomains {
            // Use a built-in small list for quick subdomain check
            subdomains = enumerate_subdomains_default(&doh_client, &domain).await;
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&DnsReport {
                domain: domain.to_string(),
                records,
                subdomains,
                warnings,
            })?
        );
    } else {
        print_records(&records);
        if !subdomains.is_empty() {
            println!();
            println!("  {}", "subdomains found:".bold().yellow());
            for sub in &subdomains {
                println!("    {}", sub);
            }
        }
        if !warnings.is_empty() {
            println!();
            println!("  {}", "warnings:".bold().yellow());
            for w in &warnings {
                println!("  {} {}", "!".yellow().bold(), w);
            }
        }
        println!();
        println!(
            "  {} records found",
            records.len().to_string().bold()
        );
        println!();
    }

    Ok(())
}

async fn fetch_doh(
    client: &reqwest::Client,
    domain: &str,
    rtype: &str,
) -> anyhow::Result<Vec<DnsRecord>> {
    let url = format!(
        "https://dns.google/resolve?name={}&type={}",
        urlencoding::encode(domain),
        rtype
    );

    let resp = client.get(&url).send().await?;
    let body: serde_json::Value = resp.json().await?;

    let mut records = Vec::new();
    if let Some(ans) = body["Answer"].as_array() {
        for a in ans {
            let name = a["name"].as_str().unwrap_or(domain).to_string();
            let data = a["data"].as_str().unwrap_or("").to_string();
            let ttl = a["TTL"].as_u64().unwrap_or(0) as u32;
            let rtype_num = a["type"].as_u64().unwrap_or(0);

            // Filter out CNAME if we asked for A (DNS returns CNAME chain)
            if rtype == "A" && rtype_num == 5 {
                continue;
            }

            records.push(DnsRecord {
                record_type: type_name(rtype_num),
                name,
                value: data,
                ttl,
            });
        }
    }

    Ok(records)
}

fn type_name(t: u64) -> String {
    match t {
        1 => "A".into(),
        2 => "NS".into(),
        5 => "CNAME".into(),
        6 => "SOA".into(),
        15 => "MX".into(),
        16 => "TXT".into(),
        28 => "AAAA".into(),
        33 => "SRV".into(),
        257 => "CAA".into(),
        _ => format!("TYPE{}", t),
    }
}

fn extract_domain(input: &str) -> &str {
    let s = input
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    s.split('/').next().unwrap_or(s)
}

fn print_records(records: &[DnsRecord]) {
    if records.is_empty() {
        println!("  no records found");
        return;
    }

    // Group by type
    let mut by_type: std::collections::BTreeMap<String, Vec<&DnsRecord>> =
        std::collections::BTreeMap::new();
    for r in records {
        by_type
            .entry(r.record_type.clone())
            .or_default()
            .push(r);
    }

    for (rtype, recs) in &by_type {
        println!("  {}", rtype.bold().yellow());
        for r in recs {
            println!(
                "    {:<20} TTL {:<8} {}",
                r.name, r.ttl, r.value
            );
        }
    }
}

fn check_security(records: &[DnsRecord], warnings: &mut Vec<String>) {
    // Check for open SPF
    for r in records {
        if r.record_type == "TXT" && r.value.contains("v=spf1") {
            if r.value.contains("+all") {
                warnings.push(format!(
                    "SPF record for {} uses +all (allows any sender)",
                    r.name
                ));
            }
            if r.value.contains("~all") {
                warnings.push(format!(
                    "SPF record for {} uses ~all (soft fail, may not block spoofing)",
                    r.name
                ));
            }
        }
    }

    // Check MX records for common providers
    let has_mx = records.iter().any(|r| r.record_type == "MX");
    if !has_mx {
        warnings.push("No MX records found — email may not be configured".into());
    }

    // Check DMARC
    let has_dmarc = records.iter().any(|r| {
        r.record_type == "TXT"
            && r.name.starts_with("_dmarc.")
            && r.value.contains("v=DMARC1")
    });
    if !has_dmarc {
        warnings.push("No DMARC record found — domain may be vulnerable to email spoofing".into());
    }
}

async fn enumerate_subdomains(
    client: &reqwest::Client,
    domain: &str,
    wordlist_path: &str,
    concurrency: u32,
) -> Vec<String> {
    let words = match std::fs::read_to_string(wordlist_path) {
        Ok(text) => {
            let mut w: Vec<String> = text
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect();
            w.truncate(10000); // safety limit
            w
        }
        Err(_) => return Vec::new(),
    };

    let found = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let next = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let mut handles = Vec::new();
    for _ in 0..concurrency.max(1) {
        let client = client.clone();
        let domain = domain.to_string();
        let words = words.clone();
        let found = found.clone();
        let next = next.clone();
        handles.push(tokio::spawn(async move {
            loop {
                let idx = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if idx >= words.len() {
                    break;
                }
                let subdomain = format!("{}.{}", words[idx], domain);
                let url = format!(
                    "https://dns.google/resolve?name={}&type=A",
                    urlencoding::encode(&subdomain)
                );
                if let Ok(resp) = client.get(&url).send().await {
                    if let Ok(body) = resp.json::<serde_json::Value>().await {
                        if let Some(ans) = body["Answer"].as_array() {
                            if !ans.is_empty() {
                                if let Ok(mut list) = found.lock() {
                                    list.push(subdomain);
                                }
                            }
                        }
                    }
                }
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }

    let result = Arc::try_unwrap(found)
        .unwrap_or_else(|arc| std::sync::Mutex::new((*arc.lock().unwrap()).clone()));
    let mut result = result.into_inner().unwrap();
    result.sort();
    result
}

async fn enumerate_subdomains_default(
    client: &reqwest::Client,
    domain: &str,
) -> Vec<String> {
    let defaults = vec![
        "www", "mail", "ftp", "localhost", "webmail", "smtp", "pop", "ns1", "ns2", "ns3", "ns4",
        "dns", "dns1", "dns2", "mx", "mx1", "mx2", "imap", "blog", "admin", "test", "dev",
        "staging", "api", "app", "portal", "vpn", "shop", "store", "cms", "cdn", "assets",
        "static", "media", "images", "img", "files", "download", "uploads", "support", "help",
        "docs", "wiki", "git", "gitlab", "github", "jenkins", "ci", "cd", "monitor", "grafana",
        "kibana", "elastic", "db", "database", "mysql", "postgres", "redis", "mongo", "minio",
        "s3", "backup", "old", "new", "v2", "v3", "beta", "alpha", "rc", "demo", "sandbox",
        "edge", "origin", "auth", "sso", "login", "accounts", "status", "health", "metrics",
        "grafana", "prometheus", "consul", "vault", "k8s", "kube", "docker", "registry",
    ];

    let found = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let next = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let concurrency = 10u32;

    let mut handles = Vec::new();
    for _ in 0..concurrency {
        let client = client.clone();
        let domain = domain.to_string();
        let defaults = defaults.clone();
        let found = found.clone();
        let next = next.clone();
        handles.push(tokio::spawn(async move {
            loop {
                let idx = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if idx >= defaults.len() {
                    break;
                }
                let subdomain = format!("{}.{}", defaults[idx], domain);
                let url = format!(
                    "https://dns.google/resolve?name={}&type=A",
                    urlencoding::encode(&subdomain)
                );
                if let Ok(resp) = client.get(&url).send().await {
                    if let Ok(body) = resp.json::<serde_json::Value>().await {
                        if let Some(ans) = body["Answer"].as_array() {
                            if !ans.is_empty() {
                                if let Ok(mut list) = found.lock() {
                                    list.push(subdomain);
                                }
                            }
                        }
                    }
                }
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }

    let result = Arc::try_unwrap(found)
        .unwrap_or_else(|arc| std::sync::Mutex::new((*arc.lock().unwrap()).clone()));
    let mut result = result.into_inner().unwrap();
    result.sort();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_domain_strips_protocol() {
        assert_eq!(extract_domain("https://example.com/"), "example.com");
        assert_eq!(extract_domain("http://example.com/path"), "example.com");
        assert_eq!(extract_domain("example.com"), "example.com");
    }

    #[test]
    fn type_name_mapping() {
        assert_eq!(type_name(1), "A");
        assert_eq!(type_name(28), "AAAA");
        assert_eq!(type_name(15), "MX");
        assert_eq!(type_name(16), "TXT");
        assert_eq!(type_name(5), "CNAME");
    }
}
