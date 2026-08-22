use colored::Colorize;
use serde::Serialize;

use crate::client::ClientConfig;
use crate::cli::CorsArgs;

#[derive(Serialize)]
struct CorsReport {
    url: String,
    tests: Vec<CorsTest>,
    vulnerable: bool,
}

#[derive(Serialize)]
struct CorsTest {
    origin: String,
    acao: Option<String>,
    acac: Option<bool>,
    aceh: Option<bool>,
    status: u16,
    risk: String,
}

const TEST_ORIGINS: &[(&str, &str)] = &[
    ("evil.com", "different domain"),
    ("null", "null origin"),
    ("https://evil.com", "https different domain"),
    ("http://evil.com", "http different domain"),
    ("subdomain.evil.com", "subdomain of attacker"),
    ("evil.com%60.evil.com", "domain boundary bypass"),
    ("evil.com%2F.evil.com", "path traversal in domain"),
    ("e\\vil.com", "backslash bypass"),
    ("evil.com\\@evil.com", "backslash before @"),
    ("evil%60.com", "backtick encoded"),
    ("evil%2560.com", "double encoded backtick"),
];

pub async fn run(args: &CorsArgs, json: bool) -> anyhow::Result<()> {
    let client = ClientConfig::from_http(&args.http)
        .without_redirects()
        .build()?;
    let mut results = Vec::new();

    for url in &args.urls {
        match test_cors(&client, url).await {
            Ok(report) => {
                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    print_report(&report);
                }
                results.push(report);
            }
            Err(e) => {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "url": url,
                            "error": e.to_string()
                        }))?
                    );
                } else {
                    println!("  {} {}: {}", "✗".red(), url, e);
                }
            }
        }
    }

    if !json && results.iter().any(|r| r.vulnerable) {
        std::process::exit(1);
    }
    Ok(())
}

async fn test_cors(client: &reqwest::Client, url: &str) -> anyhow::Result<CorsReport> {
    let mut tests = Vec::new();

    // First, test with no origin to see the baseline
    let base_resp = client.get(url).send().await?;
    let base_headers = base_resp.headers().clone();

    // If no ACAO header at all, CORS isn't configured
    if !base_headers.contains_key("access-control-allow-origin") {
        return Ok(CorsReport {
            url: url.to_string(),
            tests: vec![CorsTest {
                origin: "(no origin)".into(),
                acao: None,
                acac: None,
                aceh: None,
                status: base_resp.status().as_u16(),
                risk: "info".into(),
            }],
            vulnerable: false,
        });
    }

    for &(origin, desc) in TEST_ORIGINS {
        let resp = client
            .get(url)
            .header("Origin", origin)
            .send()
            .await?;

        let headers = resp.headers().clone();
        let status = resp.status().as_u16();

        let acao = headers
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let acac = headers
            .get("access-control-allow-credentials")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.eq_ignore_ascii_case("true"));

        let aceh = headers
            .get("access-control-allow-headers")
            .and_then(|v| v.to_str().ok())
            .map(|_| true);

        let risk = assess_risk(&acao, acac, origin);

        tests.push(CorsTest {
            origin: format!("{} ({})", origin, desc),
            acao,
            acac,
            aceh,
            status,
            risk,
        });
    }

    let vulnerable = tests.iter().any(|t| t.risk == "critical" || t.risk == "high");

    Ok(CorsReport {
        url: url.to_string(),
        tests,
        vulnerable,
    })
}

fn assess_risk(acao: &Option<String>, acac: Option<bool>, origin: &str) -> String {
    match acao.as_deref() {
        Some("*") => {
            if acac == Some(true) {
                "critical".into()
            } else {
                "medium".into()
            }
        }
        Some(reflected) if reflected == origin => {
            if acac == Some(true) {
                if origin == "null" {
                    "critical".into()
                } else {
                    "high".into()
                }
            } else {
                "medium".into()
            }
        }
        Some(_) => "low".into(),
        None => "info".into(),
    }
}

fn print_report(r: &CorsReport) {
    println!();
    println!("  {} {}", "auger cors".bold().cyan(), r.url);
    println!();

    let vuln = r.vulnerable;
    for t in &r.tests {
        let (icon, color) = match t.risk.as_str() {
            "critical" => ("✗", "red"),
            "high" => ("✗", "red"),
            "medium" => ("!", "yellow"),
            "low" => ("✓", "green"),
            _ => ("·", "white"),
        };
        let icon_colored = match color {
            "red" => icon.red().bold().to_string(),
            "yellow" => icon.yellow().bold().to_string(),
            "green" => icon.green().to_string(),
            _ => icon.dimmed().to_string(),
        };

        let acao = t.acao.as_deref().unwrap_or("-");
        let creds = match t.acac {
            Some(true) => "yes".red().to_string(),
            Some(false) => "no".green().to_string(),
            None => "-".dimmed().to_string(),
        };

        println!(
            "  {}  origin: {:<40} acao: {:<20} creds: {}  [{}]",
            icon_colored,
            t.origin,
            acao,
            creds,
            t.risk
        );
    }

    if vuln {
        println!();
        println!(
            "  {}",
            "⚠ CORS misconfiguration detected — vulnerable to cross-origin data theft"
                .red()
                .bold()
        );
    } else {
        println!();
        println!("  {}", "✓ No CORS misconfiguration detected".green());
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_assessment() {
        assert_eq!(assess_risk(&Some("*".into()), Some(true), "evil.com"), "critical");
        assert_eq!(assess_risk(&Some("*".into()), Some(false), "evil.com"), "medium");
        assert_eq!(assess_risk(&Some("evil.com".into()), Some(true), "evil.com"), "high");
        assert_eq!(assess_risk(&Some("null".into()), Some(true), "null"), "critical");
        assert_eq!(assess_risk(&Some("other.com".into()), Some(false), "evil.com"), "low");
        assert_eq!(assess_risk(&None, None, "evil.com"), "info");
    }
}
