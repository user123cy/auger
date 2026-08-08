mod check;
mod cli;
mod client;
mod color;
mod compare;
mod fmt;
mod html;
mod report;
mod runner;
mod scan;
mod stats;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.block_on(run(cli))
}

async fn run(cli: cli::Cli) -> anyhow::Result<()> {
    match cli.command {
        cli::Commands::Run(args) => {
            let report = runner::run(&args).await?;
            report::print(&report);
            if let Some(path) = &args.save {
                std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
                println!("\n  saved baseline to {}", path);
            }
            if let Some(path) = &args.compare {
                let baseline = load(path)?;
                compare::print(&baseline.stats(), &report.stats(), args.threshold);
            }
            if report.errors > 0 {
                std::process::exit(1);
            }
        }
        cli::Commands::Scan(args) => scan::run(&args).await?,
        cli::Commands::Check(args) => check::run(&args).await?,
        cli::Commands::Report { json, csv } => {
            let report = load(&json)?;
            report::print(&report);
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
            compare::print(&load(&before)?.stats(), &load(&after)?.stats(), 1.1);
        }
    }
    Ok(())
}

fn load(path: &str) -> anyhow::Result<stats::Report> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}
