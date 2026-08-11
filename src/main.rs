mod cert;
mod check;
mod cli;
mod client;
mod color;
mod compare;
mod fmt;
mod html;
mod ping;
mod report;
mod runner;
mod scan;
mod stats;
mod tls;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(run(cli))
}

async fn run(cli: cli::Cli) -> anyhow::Result<()> {
    match cli.command {
        cli::Commands::Run(args) => {
            let report = runner::run(&args, cli.json || args.quiet).await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                report::print(&report);
            }
            if let Some(path) = &args.save {
                std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
                println!("\n  saved baseline to {}", path);
            }
            let mut regression = false;
            if let Some(path) = &args.compare {
                let rows = compare::diff(&load(path)?.stats(), &report.stats(), args.threshold);
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&rows)?);
                } else {
                    compare::print_rows(&rows);
                }
                regression = rows.iter().any(|r| r.regression);
            }
            if regression || report.errors > 0 {
                std::process::exit(1);
            }
        }
        cli::Commands::Scan(args) => scan::run(&args, cli.json).await?,
        cli::Commands::Check(args) => check::run(&args, cli.json).await?,
        cli::Commands::Cert(args) => cert::run(&args.target, cli.json).await?,
        cli::Commands::Ping(args) => ping::run(&args, cli.json).await?,
        cli::Commands::Report {
            json,
            csv,
            markdown,
        } => {
            let report = load(&json)?;
            if markdown {
                report::print_markdown(&report);
            } else {
                report::print(&report);
            }
            if let Some(out) = csv {
                report::write_csv(&report, &out)?;
                println!("  wrote {}", out);
            }
        }
        cli::Commands::Html { json, out } => {
            html::export(&load(&json)?, &out)?;
            println!("  wrote {}", out);
        }
        cli::Commands::Compare { before, after } => {
            let rows = compare::diff(&load(&before)?.stats(), &load(&after)?.stats(), 1.1);
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                compare::print_rows(&rows);
            }
            if rows.iter().any(|r| r.regression) {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

fn load(path: &str) -> anyhow::Result<stats::Report> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}
