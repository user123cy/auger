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

#[cfg(feature = "tui")]
mod tui;

use clap::{CommandFactory, Parser};

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(run(cli))
}

async fn run(cli: cli::Cli) -> anyhow::Result<()> {
    match cli.command {
        cli::Commands::Run(args) => run_cmd(*args, cli.json).await?,
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
        cli::Commands::Completions { shell } => {
            let mut cmd = cli::Cli::command();
            clap_complete::generate(shell, &mut cmd, "auger", &mut std::io::stdout());
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

async fn run_cmd(args: cli::RunArgs, json: bool) -> anyhow::Result<()> {
    let urls = runner::load_urls(&args)?;

    // Battle mode: several URLs, one command, a winner.
    if urls.len() > 1 {
        if args.tui {
            anyhow::bail!("--tui supports a single URL");
        }
        if args.save.is_some() || args.compare.is_some() {
            anyhow::bail!(
                "--save and --compare need a single URL; save a baseline for each URL separately"
            );
        }
        let reports = runner::run_many(&urls, &args, json || args.quiet).await?;
        if json {
            println!("{}", serde_json::to_string_pretty(&reports)?);
        } else if args.markdown {
            report::print_markdown_matrix(&reports);
        } else {
            report::print_matrix(&reports);
        }
        if let Some(w) = &args.webhook {
            runner::post_webhook(w, &reports).await;
        }
        if reports.iter().any(|r| r.errors > 0) {
            std::process::exit(1);
        }
        return Ok(());
    }

    let url = urls[0].clone();

    #[cfg(feature = "tui")]
    if args.tui {
        let report = runner::run_tui(url, &args).await?;
        return finish_run(report, &args, json).await;
    }
    #[cfg(not(feature = "tui"))]
    if args.tui {
        eprintln!(
            "TUI mode requires the 'tui' feature. Install with: cargo install auger --features tui"
        );
        std::process::exit(1);
    }

    let report = runner::run(url, &args, json || args.quiet).await?;
    finish_run(report, &args, json).await
}

async fn finish_run(report: stats::Report, args: &cli::RunArgs, json: bool) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if args.markdown {
        report::print_markdown(&report);
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
        if json {
            println!("{}", serde_json::to_string_pretty(&rows)?);
        } else {
            compare::print_rows(&rows);
        }
        regression = rows.iter().any(|r| r.regression);
    }
    if let Some(w) = &args.webhook {
        runner::post_webhook(w, std::slice::from_ref(&report)).await;
    }
    if regression || report.errors > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn load(path: &str) -> anyhow::Result<stats::Report> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}
