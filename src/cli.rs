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
    Run(Box<RunArgs>),
    /// Probe a wordlist against a base URL to discover endpoints
    Scan(ScanArgs),
    /// Inspect a URL: status, HTTP version and security headers
    Check(CheckArgs),
    /// Show the TLS certificate for a host
    Cert(CertArgs),
    /// Measure per-phase latency of a single HTTP request
    Ping(PingArgs),
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
    /// Detect technologies, frameworks and CMS from a URL
    Tech(CheckArgs),
    /// Test CORS misconfigurations on a URL
    Cors(CorsArgs),
    /// Enumerate DNS records and check security
    Dns(DnsArgs),
    /// Fuzz HTTP endpoints with payload injection
    Fuzz(FuzzArgs),
    /// Chaos engineering: inject failures and measure server resilience
    Chaos(ChaosArgs),
    /// Behavioral fingerprinting: create unique server signature
    Fingerprint(FingerprintArgs),
    /// Generate narrative story from a saved load test report
    Story(StoryArgs),
    /// Generate shell completion scripts
    Completions {
        /// Shell to generate for: bash, zsh, fish, powershell, elvish
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
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
    /// URL(s) to load test — pass several to compare them in a battle matrix
    #[arg(num_args = 1.., required_unless_present_any = ["stdin", "urls_file"])]
    pub urls: Vec<String>,

    /// Read more URLs from this file, one per line
    #[arg(long)]
    pub urls_file: Option<String>,

    /// Read more URLs from stdin, one per line
    #[arg(long)]
    pub stdin: bool,

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

    /// Enable real-time TUI dashboard (requires 'tui' feature)
    #[arg(long)]
    pub tui: bool,

    /// Status codes or classes counted as success, comma separated, e.g. 2xx,3xx,401
    #[arg(long, default_value = "2xx,3xx")]
    pub status_ok: String,

    /// Discard the first seconds of the run from the statistics, e.g. 3s
    #[arg(long)]
    pub warmup: Option<String>,

    /// Abort the run after this many errors
    #[arg(long)]
    pub max_errors: Option<u64>,

    /// Print the report as a markdown table instead of the text report
    #[arg(long)]
    pub markdown: bool,

    /// Post the result summary to a webhook (Discord or Slack) when the run finishes
    #[arg(long)]
    pub webhook: Option<String>,

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

    /// Exclude these status codes, comma separated, e.g. 403,500
    #[arg(long)]
    pub filter_status: Option<String>,

    /// Exclude responses with exactly this body size in bytes
    #[arg(long)]
    pub filter_size: Option<u64>,

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

#[derive(clap::Args)]
pub struct PingArgs {
    /// URL to ping, e.g. https://example.com/
    pub url: String,

    /// Number of requests to send
    #[arg(short = 'c', long, default_value_t = 1)]
    pub count: u32,

    #[command(flatten)]
    pub http: HttpOptions,
}

#[derive(clap::Args)]
pub struct CorsArgs {
    /// URL(s) to test for CORS misconfigurations
    #[arg(num_args = 1..)]
    pub urls: Vec<String>,

    #[command(flatten)]
    pub http: HttpOptions,
}

#[derive(clap::Args)]
pub struct DnsArgs {
    /// Domain or URL to query
    pub domain: String,

    /// Record types to query, e.g. A,MX,TXT
    #[arg(short, long, default_value = "A")]
    pub types: Vec<String>,

    /// Query all common record types
    #[arg(long)]
    pub all: bool,

    /// Try subdomain enumeration
    #[arg(long)]
    pub subdomains: bool,

    /// Wordlist for subdomain enumeration
    #[arg(long)]
    pub wordlist: Option<String>,

    /// Concurrency for subdomain enumeration
    #[arg(short, long, default_value_t = 10)]
    pub concurrency: u32,

    /// Show verbose output including empty results
    #[arg(long)]
    pub verbose: bool,
}

#[derive(clap::Args)]
pub struct FuzzArgs {
    /// Target URL (use FUZZ as placeholder)
    pub url: String,

    /// Wordlist file with payloads
    #[arg(short, long)]
    pub wordlist: Option<String>,

    /// Use built-in payloads (path traversal, XSS, SQLi, etc.)
    #[arg(long)]
    pub builtin: bool,

    /// Injection point: path, query, body, header, subdomain, wordlist
    #[arg(short = 'i', long, default_value = "path")]
    pub injection: String,

    /// HTTP method
    #[arg(short, long, default_value = "GET")]
    pub method: String,

    /// Request body (use FUZZ as placeholder)
    #[arg(long)]
    pub body: Option<String>,

    #[arg(short, long, default_value_t = 20)]
    pub concurrency: u32,

    /// Maximum number of payloads to test
    #[arg(long)]
    pub max_payloads: Option<u64>,

    /// Only report these status codes, comma separated
    #[arg(long)]
    pub filter_status: Option<String>,

    #[command(flatten)]
    pub http: HttpOptions,
}

#[derive(clap::Args)]
pub struct ChaosArgs {
    /// Target URL to chaos-test
    pub url: String,

    /// Number of rounds per phase
    #[arg(short, long, default_value_t = 50)]
    pub rounds: u32,

    /// Delay in ms between phases
    #[arg(long, default_value_t = 1000)]
    pub delay_ms: u64,

    #[command(flatten)]
    pub http: HttpOptions,
}

#[derive(clap::Args)]
pub struct FingerprintArgs {
    /// URL to fingerprint
    pub url: String,

    #[command(flatten)]
    pub http: HttpOptions,
}

#[derive(clap::Args)]
pub struct StoryArgs {
    /// Path to a saved report JSON file
    pub report: String,
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

/// Read one item per line, trimming whitespace and dropping blank lines.
pub fn read_urls<R: std::io::BufRead>(r: R) -> Vec<String> {
    read_lines(r, false)
}

/// Like `read_urls` but also drops `#` comment lines (used for wordlists).
pub fn read_words<R: std::io::BufRead>(r: R) -> Vec<String> {
    read_lines(r, true)
}

fn read_lines<R: std::io::BufRead>(r: R, skip_comments: bool) -> Vec<String> {
    r.lines()
        .map_while(Result::ok)
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !(skip_comments && l.starts_with('#')))
        .collect()
}
