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
            out.push(Bucket {
                lo,
                hi,
                count: counts[i],
            });
        }
        lo = hi;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(latencies: Vec<f64>) -> Report {
        Report {
            url: "http://test".into(),
            concurrency: 1,
            elapsed_ms: 1000,
            requests: latencies.len() as u64,
            errors: 0,
            bytes: 0,
            latencies_ms: latencies,
            ttfb_ms: vec![],
            statuses: Default::default(),
            errors_timeout: 0,
            errors_connect: 0,
            errors_tls: 0,
            errors_other: 0,
            slowest_ms: vec![],
        }
    }

    #[test]
    fn percentiles_on_known_data() {
        let s = report(vec![10.0, 20.0, 30.0, 40.0, 50.0]).stats();
        assert_eq!(s.p50, 30.0);
        assert_eq!(s.p75, 40.0);
        assert_eq!(s.p90, 50.0);
        assert_eq!(s.p95, 50.0);
        assert_eq!(s.p99, 50.0);
        assert_eq!(s.max_ms, 50.0);
        assert_eq!(s.mean_ms, 30.0);
    }

    #[test]
    fn single_sample() {
        let s = report(vec![42.0]).stats();
        assert_eq!(s.p50, 42.0);
        assert_eq!(s.max_ms, 42.0);
        assert_eq!(s.mean_ms, 42.0);
    }

    #[test]
    fn empty_report_zero_stats() {
        let s = report(vec![]).stats();
        assert_eq!(s.p50, 0.0);
        assert_eq!(s.mean_ms, 0.0);
        assert_eq!(s.max_ms, 0.0);
        assert!(s.histogram.is_empty());
    }

    #[test]
    fn rps_uses_elapsed_ms() {
        let r = report(vec![1.0, 2.0]);
        assert_eq!(r.stats().rps, 2.0);
    }

    #[test]
    fn histogram_keeps_every_sample() {
        let s = report(vec![1.0, 2.0, 3.0]).stats();
        let total: u64 = s.histogram.iter().map(|b| b.count).sum();
        assert_eq!(total, 3);
        assert!(s.histogram.iter().all(|b| b.lo < b.hi));
    }

    #[test]
    fn report_round_trip() {
        let r = report(vec![1.5, 2.5, 3.5]);
        let json = serde_json::to_string(&r).unwrap();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(back.url, r.url);
        assert_eq!(back.requests, r.requests);
        assert_eq!(back.latencies_ms, r.latencies_ms);
    }
}
