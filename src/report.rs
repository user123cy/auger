use colored::Colorize;

use crate::color::heat;
use crate::fmt::{group, ms};
use crate::stats::{Report, Stats};

pub fn print(report: &Report) {
    let stats = report.stats();
    print_summary(report, &stats);
    print_percentiles(&stats);
    if stats.histogram.is_empty() {
        return;
    }
    print_histogram(&stats);
    print_flame(&stats);
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
    if report.bytes > 0 {
        line.push_str(&format!(" · {:.1} MB downloaded", report.bytes as f64 / 1_000_000.0));
    }
    println!("{}", line);
    println!();
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
    let max_count = stats.histogram.iter().map(|b| b.count).max().unwrap_or(1).max(1) as f64;
    println!();
    println!("  latency histogram (ms)");
    for b in &stats.histogram {
        let bar_len = (b.count as f64 / max_count * 30.0).round() as usize;
        let bar = "█".repeat(bar_len.max(1));
        let label = format!("{:.1}–{:.1}", b.lo, b.hi);
        let (r, g, bl) = heat((b.hi / stats.max_ms.max(1.0)).clamp(0.0, 1.0));
        println!("  {:>10} {} {}", label, bar.truecolor(r, g, bl), group(b.count));
    }
}

fn print_flame(stats: &Stats) {
    println!();
    println!("  latency flamegraph");
    let height = 8u32;
    let max_count = stats.histogram.iter().map(|b| b.count).max().unwrap_or(1).max(1) as f64;
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