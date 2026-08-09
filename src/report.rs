use colored::Colorize;

use crate::color::heat;
use crate::fmt::{dec1, group, ms, whole};
use crate::stats::{Report, Stats};

pub fn print_markdown(report: &Report) {
    let stats = report.stats();
    println!("# auger — {}", report.url);
    println!();
    println!("| metric | value |");
    println!("|---|---|");
    println!("| requests | {} |", group(report.requests));
    println!("| req/s | {} |", whole(stats.rps));
    println!("| errors | {} |", group(report.errors));
    println!(
        "| duration | {} s |",
        dec1(report.elapsed_ms as f64 / 1000.0)
    );
    println!("| mean | {} ms |", dec1(stats.mean_ms));
    println!("| p50 | {} ms |", dec1(stats.p50));
    println!("| p75 | {} ms |", dec1(stats.p75));
    println!("| p90 | {} ms |", dec1(stats.p90));
    println!("| p95 | {} ms |", dec1(stats.p95));
    println!("| p99 | {} ms |", dec1(stats.p99));
    println!("| max | {} ms |", dec1(stats.max_ms));
}

pub fn print(report: &Report) {
    let stats = report.stats();
    print_summary(report, &stats);
    print_percentiles(&stats);
    if !stats.histogram.is_empty() {
        print_histogram(&stats);
        print_flame(&stats);
    }
    print_ttfb(&report.ttfb_ms);
    print_slowest(&report.slowest_ms);
}

fn print_summary(report: &Report, stats: &Stats) {
    let secs = report.elapsed_ms as f64 / 1000.0;
    println!();
    println!("  {} {}", "auger".bold().cyan(), report.url);
    println!(
        "  {} workers · {}s · {} req · {} req/s",
        report.concurrency,
        secs,
        group(report.requests),
        group(stats.rps.round() as u64)
    );
    let mut line = format!("  {} errors", group(report.errors));
    let e = report.errors_timeout;
    if e > 0 {
        line.push_str(&format!(" · {} timeout", group(e)));
    }
    let e = report.errors_connect;
    if e > 0 {
        line.push_str(&format!(" · {} conn refused", group(e)));
    }
    let e = report.errors_tls;
    if e > 0 {
        line.push_str(&format!(" · {} tls", group(e)));
    }
    let e = report.errors_other;
    if e > 0 {
        line.push_str(&format!(" · {} other", group(e)));
    }
    if report.bytes > 0 {
        line.push_str(&format!(
            " · {:.1} MB downloaded",
            report.bytes as f64 / 1_000_000.0
        ));
    }
    println!("{}", line);

    if !report.statuses.is_empty() {
        let parts: Vec<String> = report
            .statuses
            .iter()
            .map(|(k, v)| format!("{} x{}", k, group(*v)))
            .collect();
        println!("  status  {}", parts.join("  "));
    }
    for e in &report.first_errors {
        println!("  error  {}", e);
    }
    println!();
}

fn print_ttfb(ttfb: &[f64]) {
    if ttfb.is_empty() {
        return;
    }
    let mut s = ttfb.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct = |p: f64| s[((s.len() - 1) as f64 * p).round() as usize];
    println!();
    println!(
        "  ttfb (ms)  p50 {} · p95 {} · max {}",
        ms(pct(0.50)),
        ms(pct(0.95)),
        ms(*s.last().unwrap())
    );
}

fn print_slowest(top: &[f64]) {
    if top.is_empty() {
        return;
    }
    println!();
    println!("  slowest (ms)");
    for (i, v) in top.iter().enumerate() {
        println!("    {}  {}", i + 1, ms(*v));
    }
}

pub fn write_csv(report: &Report, path: &str) -> anyhow::Result<()> {
    let mut s = String::from("latency_ms\n");
    for v in &report.latencies_ms {
        s.push_str(&format!("{:.3}\n", v));
    }
    std::fs::write(path, s)?;
    Ok(())
}

fn print_percentiles(stats: &Stats) {
    println!("  percentiles (ms)");
    let rows = [
        ("p50", stats.p50),
        ("p75", stats.p75),
        ("p90", stats.p90),
        ("p95", stats.p95),
        ("p99", stats.p99),
        ("max", stats.max_ms),
    ];
    for (label, v) in rows {
        let f = (v / stats.max_ms.max(1.0)).clamp(0.0, 1.0);
        let (r, g, b) = heat(if label == "max" { 1.0 } else { f });
        println!("   {:<4} {}", label.truecolor(r, g, b).bold(), ms(v));
    }
}

fn print_histogram(stats: &Stats) {
    let max_count = stats
        .histogram
        .iter()
        .map(|b| b.count)
        .max()
        .unwrap_or(1)
        .max(1) as f64;
    println!();
    println!("  latency histogram (ms)");
    for b in &stats.histogram {
        let bar_len = (b.count as f64 / max_count * 30.0).round() as usize;
        let bar = "█".repeat(bar_len.max(1));
        let label = format!("{:.1}–{:.1}", b.lo, b.hi);
        let (r, g, bl) = heat((b.hi / stats.max_ms.max(1.0)).clamp(0.0, 1.0));
        println!(
            "  {:>10} {} {}",
            label,
            bar.truecolor(r, g, bl),
            group(b.count)
        );
    }
}

fn print_flame(stats: &Stats) {
    println!();
    println!("  latency flamegraph");
    let height = 8u32;
    let max_count = stats
        .histogram
        .iter()
        .map(|b| b.count)
        .max()
        .unwrap_or(1)
        .max(1) as f64;
    for row in (0..height).rev() {
        let mut line = String::from("   ");
        for b in &stats.histogram {
            let h = (b.count as f64 / max_count * height as f64).round() as u32;
            let (r, g, bl) = heat((b.hi / stats.max_ms.max(1.0)).clamp(0.0, 1.0));
            if h > row {
                line.push_str(&"█".truecolor(r, g, bl).to_string());
            } else {
                line.push(' ');
            }
        }
        println!("{}", line);
    }
    let width = stats.histogram.len().max(1);
    println!("   0{} {} ms", "─".repeat(width), ms(stats.max_ms));
}
