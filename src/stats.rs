use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Report {
    pub url: String,
    pub concurrency: u32,
    pub elapsed_ms: u64,
    pub requests: u64,
    #[serde(default)]
    pub warmup_discarded: u64,
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
    pub errors_status: u64,
    #[serde(default)]
    pub statuses: BTreeMap<u16, u64>,
    #[serde(default)]
    pub ttfb_ms: Vec<f64>,
    #[serde(default)]
    pub slowest_ms: Vec<f64>,
    #[serde(default)]
    pub first_errors: Vec<String>,
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
    #[cfg(feature = "tui")]
    pub fn new(url: String) -> Self {
        Self {
            url,
            concurrency: 0,
            elapsed_ms: 0,
            requests: 0,
            warmup_discarded: 0,
            errors: 0,
            bytes: 0,
            latencies_ms: Vec::new(),
            errors_timeout: 0,
            errors_connect: 0,
            errors_tls: 0,
            errors_other: 0,
            errors_status: 0,
            statuses: BTreeMap::new(),
            ttfb_ms: Vec::new(),
            slowest_ms: Vec::new(),
            first_errors: Vec::new(),
        }
    }

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

// Index of the "fastest" report in a battle: lowest p50, ties broken by
// req/s. Only reports with at least one request can win (a dead endpoint has
// p50 == 0.0), and healthy reports (no errors) are preferred over erroring
// ones — a 404 endpoint answering fast is not a winner.
pub fn winner(reports: &[Report]) -> Option<usize> {
    let usable: Vec<usize> = reports
        .iter()
        .enumerate()
        .filter(|(_, r)| r.requests > 0)
        .map(|(i, _)| i)
        .collect();
    if usable.is_empty() {
        return None;
    }
    let clean: Vec<usize> = usable
        .iter()
        .copied()
        .filter(|&i| reports[i].errors == 0)
        .collect();
    let pool = if clean.is_empty() { &usable } else { &clean };
    pool.iter().copied().min_by(|&i, &j| {
        let a = reports[i].stats();
        let b = reports[j].stats();
        a.p50
            .partial_cmp(&b.p50)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.rps
                    .partial_cmp(&a.rps)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    })
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
            warmup_discarded: 0,
            errors: 0,
            bytes: 0,
            latencies_ms: latencies,
            ttfb_ms: vec![],
            statuses: Default::default(),
            errors_timeout: 0,
            errors_connect: 0,
            errors_tls: 0,
            errors_other: 0,
            errors_status: 0,
            slowest_ms: vec![],
            first_errors: vec![],
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
    fn winner_picks_lowest_p50_then_higher_rps() {
        let a = report(vec![5.0, 6.0]);
        let b = report(vec![10.0, 11.0]);
        assert_eq!(winner(&[a.clone(), b.clone()]), Some(0));
        assert_eq!(winner(&[b, a.clone()]), Some(1));
        // Same p50: higher req/s wins the tie.
        let mut fast = report(vec![5.0, 6.0]);
        fast.elapsed_ms = 500; // 2 samples in 0.5s -> 4 req/s vs 2 req/s
        assert_eq!(winner(&[a, fast]), Some(1));
    }

    #[test]
    fn dead_endpoint_never_wins() {
        let dead = report(vec![]); // 0 requests -> p50 would be 0.0
        let ok = report(vec![30.0, 40.0]);
        assert_eq!(winner(&[dead.clone(), ok.clone()]), Some(1));
        assert_eq!(winner(&[ok, dead.clone()]), Some(0));
        assert_eq!(winner(&[dead.clone(), dead.clone()]), None);
    }

    #[test]
    fn winner_prefers_healthy_over_erroring() {
        let healthy = report(vec![50.0, 60.0]);
        let mut erroring = report(vec![1.0, 2.0]); // faster, but erroring
        erroring.errors = 10;
        assert_eq!(winner(&[erroring.clone(), healthy.clone()]), Some(1));
        // If every report errors, fall back to fastest anyway.
        let mut other = report(vec![5.0, 6.0]);
        other.errors = 2;
        assert_eq!(winner(&[erroring, other]), Some(0));
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
