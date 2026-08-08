use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Report {
    pub url: String,
    pub concurrency: u32,
    pub elapsed_ms: u64,
    pub requests: u64,
    pub errors: u64,
    pub bytes: u64,
    pub latencies_ms: Vec<f64>,
    #[serde(default)]
    pub errors_timeout: u64,
    #[serde(default)]
    pub errors_connect: u64,
    #[serde(default)]
    pub errors_tls: u64,
    #[serde(default)]
    pub errors_other: u64,
    #[serde(default)]
    pub statuses: BTreeMap<u16, u64>,
    #[serde(default)]
    pub ttfb_ms: Vec<f64>,
    #[serde(default)]
    pub slowest_ms: Vec<f64>,
}

#[derive(Debug)]
pub struct Stats {
    pub mean_ms: f64,
    pub p50: f64,
    pub p75: f64,
    pub p90: f64,
    pub p95: f64,
    pub p99: f64,
    pub max_ms: f64,
    pub rps: f64,
    pub histogram: Vec<Bucket>,
}

#[derive(Debug)]
pub struct Bucket {
    pub lo: f64,
    pub hi: f64,
    pub count: u64,
}

impl Report {
    pub fn stats(&self) -> Stats {
        let mut sorted = self.latencies_ms.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let pct = |p: f64| -> f64 {
            if sorted.is_empty() {
                0.0
            } else {
                let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
                sorted[idx]
            }
        };
        let mean = if sorted.is_empty() {
            0.0
        } else {
            sorted.iter().sum::<f64>() / sorted.len() as f64
        };
        let max = sorted.last().copied().unwrap_or(0.0);
        let rps = if self.elapsed_ms > 0 {
            self.requests as f64 / (self.elapsed_ms as f64 / 1000.0)
        } else {
            0.0
        };

        Stats {
            mean_ms: mean,
            p50: pct(0.50),
            p75: pct(0.75),
            p90: pct(0.90),
            p95: pct(0.95),
            p99: pct(0.99),
            max_ms: max,
            rps,
            histogram: histogram(&sorted, max),
        }
    }
}

// Buckets grow on a log scale so a few slow outliers don't drown the shape.
fn histogram(sorted: &[f64], max: f64) -> Vec<Bucket> {
    if sorted.is_empty() {
        return Vec::new();
    }
    let mut edges = Vec::new();
    let mut e = 0.25;
    while e < max * 1.5 {
        edges.push(e);
        e *= 1.6;
    }
    edges.push(f64::INFINITY);

    let mut counts = vec![0u64; edges.len()];
    for &v in sorted {
        let mut i = 0;
        while i < edges.len() - 1 && v >= edges[i] {
            i += 1;
        }
        counts[i] += 1;
    }

    let mut lo = 0.0;
    let mut out = Vec::new();
    for (i, &hi) in edges.iter().enumerate() {
        if counts[i] > 0 {
            let hi = if hi.is_infinite() { max } else { hi };
            out.push(Bucket { lo, hi, count: counts[i] });
        }
        lo = hi;
    }
    out
}
