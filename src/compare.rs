use colored::Colorize;

use crate::fmt::{ms, whole};
use crate::stats::Stats;

pub struct DiffRow {
    pub label: &'static str,
    pub before: f64,
    pub after: f64,
    pub regression: bool,
    pub improved: bool,
    higher_is_better: bool,
}

pub fn diff(before: &Stats, after: &Stats, threshold: f64) -> Vec<DiffRow> {
    let t = threshold.max(1.0);
    let mut rows = vec![
        row("mean", before.mean_ms, after.mean_ms),
        row("p50", before.p50, after.p50),
        row("p75", before.p75, after.p75),
        row("p90", before.p90, after.p90),
        row("p95", before.p95, after.p95),
        row("p99", before.p99, after.p99),
        row("max", before.max_ms, after.max_ms),
    ];
    rows.push(DiffRow {
        label: "req/s",
        before: before.rps,
        after: after.rps,
        regression: false,
        improved: false,
        higher_is_better: true,
    });
    for r in &mut rows {
        r.regression = if r.higher_is_better {
            r.after < r.before / t
        } else {
            r.after > r.before * t
        };
        r.improved = if r.higher_is_better {
            r.after > r.before * t
        } else {
            r.after < r.before / t
        };
    }
    rows
}

fn row(label: &'static str, before: f64, after: f64) -> DiffRow {
    DiffRow {
        label,
        before,
        after,
        regression: false,
        improved: false,
        higher_is_better: false,
    }
}

pub fn print(before: &Stats, after: &Stats, threshold: f64) {
    let rows = diff(before, after, threshold);
    println!();
    println!("  compare — baseline vs current");
    println!(
        "  {:<6} {:>10} {:>10} {:>9}",
        "metric", "before", "after", "change"
    );
    for r in &rows {
        let pct = if r.before > 0.0 {
            (r.after / r.before - 1.0) * 100.0
        } else {
            0.0
        };
        let change = format!("{:+.1}%", pct);
        let value = if r.higher_is_better {
            whole(r.after)
        } else {
            ms(r.after)
        };
        let line = format!(
            "  {:<6} {:>10} {:>10} {:>9}",
            r.label,
            if r.higher_is_better {
                whole(r.before)
            } else {
                ms(r.before)
            },
            value,
            change,
        );
        if r.regression {
            println!("{}", line.red().bold());
        } else if r.improved {
            println!("{}", line.green());
        } else {
            println!("{}", line);
        }
    }
    if rows.iter().any(|r| r.regression) {
        println!("  {}", "⚠ regression detected".red().bold());
    } else {
        println!("  {}", "no regression".green());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(mean: f64, p50: f64, rps: f64) -> Stats {
        Stats {
            mean_ms: mean,
            p50,
            p75: 0.0,
            p90: 0.0,
            p95: 0.0,
            p99: 0.0,
            max_ms: 0.0,
            rps,
            histogram: vec![],
        }
    }

    #[test]
    fn latency_slower_flags_regression() {
        let rows = diff(&mk(10.0, 10.0, 0.0), &mk(20.0, 20.0, 0.0), 1.5);
        let mean = rows.iter().find(|r| r.label == "mean").unwrap();
        assert!(mean.regression);
        assert!(!mean.improved);
    }

    #[test]
    fn latency_faster_flags_improvement() {
        let rows = diff(&mk(20.0, 20.0, 0.0), &mk(10.0, 10.0, 0.0), 1.5);
        let mean = rows.iter().find(|r| r.label == "mean").unwrap();
        assert!(mean.improved);
        assert!(!mean.regression);
    }

    #[test]
    fn throughput_lower_flags_regression() {
        let rows = diff(&mk(0.0, 0.0, 100.0), &mk(0.0, 0.0, 50.0), 1.5);
        let rps = rows.iter().find(|r| r.label == "req/s").unwrap();
        assert!(rps.regression);
    }

    #[test]
    fn small_change_within_threshold() {
        let rows = diff(&mk(10.0, 10.0, 100.0), &mk(12.0, 12.0, 95.0), 1.5);
        assert!(rows.iter().all(|r| !r.regression && !r.improved));
    }

    #[test]
    fn threshold_below_one_is_clamped() {
        let rows = diff(&mk(10.0, 10.0, 0.0), &mk(12.0, 12.0, 0.0), 0.5);
        let mean = rows.iter().find(|r| r.label == "mean").unwrap();
        // t is clamped to 1.0, so 12 > 10 flags a regression
        assert!(mean.regression);
    }
}
