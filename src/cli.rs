use std::time::Duration;

use anyhow::Context;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "auger",
    version,
    about = "Load test, discover and inspect HTTP endpoints"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Emit machine-readable JSON to stdout instead of the table
    #[arg(global = true, long, id = "json_out")]
    pub json: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run a load test against a URL
    Run(RunArgs),
    /// Probe a wordlist against a base URL to discover endpoints
    Scan(ScanArgs),
    /// Inspect a URL: status, HTTP version and security headers
    Check(CheckArgs),
    /// Show the TLS certificate for a host
    Cert(CertArgs),
    /// Print a saved report
    Report {
        json: String,
        /// Also write latencies as CSV
        #[arg(long)]
        csv: Option<String>,
        /// Print the report as a markdown table
        #[arg(long)]
        markdown: bool,
    },
    /// Export a saved report to a self-contained HTML file
    Html {
        json: String,
        #[arg(short, long, default_value = "report.html")]
        out: String,
    },
    /// Compare two saved reports
    Compare { before: String, after: String },
}

#[derive(clap::Args)]
pub struct HttpOptions {
    #[arg(short = 'k', long)]
    pub insecure: bool,

    /// HTTP proxy, e.g. http://127.0.0.1:8080
    #[arg(short, long)]
    pub proxy: Option<String>,

    #[arg(short, long, default_value_t = 30)]
    pub timeout: u64,

    #[arg(short = 'A', long)]
    pub user_agent: Option<String>,

    /// Request header, repeatable: -H "X-Token: abc"
    #[arg(short = 'H', long = "header")]
    pub headers: Vec<String>,

    /// Use HTTP/2 only
    #[arg(long)]
    pub http2: bool,

    /// Disable connection keep-alive
    #[arg(long)]
    pub no_keepalive: bool,
}

#[derive(clap::Args)]
pub struct RunArgs {
    pub url: String,

    #[arg(short, long, default_value_t = 20)]
    pub concurrency: u32,

    /// How long to run, e.g. 5s, 30s, 2m
    #[arg(short, long, default_value = "5s")]
    pub duration: String,

    #[arg(short, long, default_value = "GET")]
    pub method: String,

    /// Save the raw result to a JSON file, usable as a baseline
    #[arg(short, long)]
    pub save: Option<String>,

    /// Compare against a saved baseline and flag regressions
    #[arg(long)]
    pub compare: Option<String>,

    /// Flag a regression when a percentile is slower by more than this factor
    #[arg(long, default_value_t = 1.1)]
    pub threshold: f64,

    /// Rotate between common user agents per worker
    #[arg(long)]
    pub random_ua: bool,

    /// Basic auth in the form user:pass
    #[arg(long)]
    pub basic: Option<String>,

    /// Send an Authorization: Bearer token
    #[arg(long)]
    pub token: Option<String>,

    /// Run a fixed number of requests instead of by time
    #[arg(short = 'n', long)]
    pub requests: Option<u64>,

    /// Ramp up concurrency over this duration, e.g. 30s
    #[arg(long)]
    pub ramp: Option<String>,

    /// Cap total requests per second
    #[arg(long)]
    pub rps: Option<u64>,

    /// Read the request body from a file
    #[arg(long)]
    pub body_file: Option<String>,

    /// Send this string as the request body (for POST/PUT)
    #[arg(long)]
    pub body: Option<String>,

    /// Suppress the per-second progress line
    #[arg(long)]
    pub quiet: bool,

    #[command(flatten)]
    pub http: HttpOptions,
}

#[derive(clap::Args)]
pub struct ScanArgs {
    /// Base URL to probe, e.g. https://target.com/
    #[arg(required_unless_present = "stdin", conflicts_with = "stdin")]
    pub url: Option<String>,

    #[arg(short, long)]
    pub wordlist: String,

    /// Append these extensions when the word has none, e.g. php,html
    #[arg(short, long)]
    pub extensions: Option<String>,

    #[arg(short, long, default_value_t = 20)]
    pub concurrency: u32,

    #[arg(short, long)]
    pub output: Option<String>,

    /// Show the <title> of 2xx pages
    #[arg(long)]
    pub title: bool,

    /// Only show these status codes, comma separated
    #[arg(long)]
    pub match_status: Option<String>,

    /// Delay in ms between requests per worker
    #[arg(long, default_value_t = 0)]
    pub delay: u64,

    /// Also probe paths found in robots.txt and sitemap.xml
    #[arg(short = 'R', long)]
    pub robots: bool,

    /// Disable recursion into 2xx directories
    #[arg(long)]
    pub no_recursion: bool,

    /// Recursion depth for 2xx directories
    #[arg(long, default_value_t = 3)]
    pub depth: u32,

    /// Read base URLs from stdin, one per line
    #[arg(long)]
    pub stdin: bool,

    /// Print only matching paths ("status url" per line)
    #[arg(long)]
    pub silent: bool,

    #[command(flatten)]
    pub http: HttpOptions,
}

#[derive(clap::Args)]
pub struct CertArgs {
    /// Host or URL, e.g. example.com:8443
    pub target: String,
}

#[derive(clap::Args)]
pub struct CheckArgs {
    pub url: String,

    /// Read URLs from a file, one per line
    #[arg(short = 'f', long)]
    pub file: Option<String>,

    #[command(flatten)]
    pub http: HttpOptions,
}

pub fn parse_duration(raw: &str) -> anyhow::Result<Duration> {
    let raw = raw.trim();
    let (num, unit) = raw.split_at(raw.len().saturating_sub(1));
    let n: u64 = num
        .trim()
        .parse()
        .with_context(|| format!("bad duration '{}'", raw))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        _ => anyhow::bail!("unknown duration unit in '{}', use 5s, 2m or 1h", raw),
    };
    Ok(Duration::from_secs(secs))
}
