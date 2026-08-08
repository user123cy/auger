use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use colored::Colorize;

use crate::cli::ScanArgs;
use crate::client::ClientConfig;
use crate::fmt::group;

struct Found {
    url: String,
    status: u16,
    size: u64,
    ms: f64,
}

pub async fn run(args: &ScanArgs) -> anyhow::Result<()> {
    let words = expand(load_words(&args.wordlist)?, args.extensions.as_deref());
    if words.is_empty() {
        anyhow::bail!("wordlist '{}' has no entries", args.wordlist);
    }

    let words = Arc::new(words);
    let next = Arc::new(AtomicUsize::new(0));
    let workers = args.concurrency.max(1);
    let start = Instant::now();

    ClientConfig::from_http(&args.http).build()?;

    let mut handles = Vec::new();
    for i in 0..workers {
        let words = words.clone();
        let next = next.clone();
        let base = args.url.clone();
        let config = ClientConfig::from_http(&args.http).worker(i as usize).without_redirects();
        handles.push(tokio::spawn(async move {
            let client = config.build()?;
            let mut found = Vec::new();
            loop {
                let idx = next.fetch_add(1, Ordering::Relaxed);
                if idx >= words.len() {
                    break;
                }
                let target = join(&base, &words[idx]);
                let t0 = Instant::now();
                match client.get(&target).send().await {
                    Ok(resp) => found.push(Found {
                        url: target,
                        status: resp.status().as_u16(),
                        size: resp.content_length().unwrap_or(0),
                        ms: t0.elapsed().as_secs_f64() * 1000.0,
                    }),
                    Err(_) => {}
                }
            }
            Ok::<_, anyhow::Error>(found)
        }));
    }

    let mut found = Vec::new();
    for h in handles {
        if let Ok(Ok(mut f)) = h.await {
            found.append(&mut f);
        }
    }
    found.sort_by(|a, b| a.status.cmp(&b.status).then(a.url.cmp(&b.url)));

    println!();
    println!("  {} {}", "auger scan".bold().cyan(), args.url);
    for f in found.iter().filter(|f| f.status != 404) {
        println!("{}", row(f));
    }
    let hits = found.iter().filter(|f| f.status != 404).count();
    println!();
    println!(
        "  {} paths · {} found · {:.1}s",
        group(words.len() as u64),
        group(hits as u64),
        start.elapsed().as_secs_f64()
    );

    if let Some(out) = args.output.as_deref() {
        let mut data = String::new();
        for f in found.iter().filter(|f| f.status != 404) {
            data.push_str(&format!("{} {}\n", f.status, f.url));
        }
        std::fs::write(out, data)?;
        println!("  wrote {} paths to {}", hits, out);
    }
    Ok(())
}

fn row(f: &Found) -> String {
    let line = format!(
        "  {:>3} {:>9} {:>8.0}ms  {}",
        f.status,
        size_str(f.size),
        f.ms,
        f.url
    );
    match f.status {
        200..=299 => line.green().to_string(),
        300..=399 => line.cyan().to_string(),
        400..=499 => line.yellow().bold().to_string(),
        _ => line.red().to_string(),
    }
}

fn size_str(b: u64) -> String {
    if b == 0 {
        "-".into()
    } else if b < 1024 {
        format!("{}B", b)
    } else if b < 1024 * 1024 {
        format!("{:.1}K", b as f64 / 1024.0)
    } else {
        format!("{:.1}M", b as f64 / (1024.0 * 1024.0))
    }
}

fn load_words(path: &str) -> anyhow::Result<Vec<String>> {
    let text = std::fs::read_to_string(path)?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect())
}

fn expand(words: Vec<String>, extensions: Option<&str>) -> Vec<String> {
    let Some(exts) = extensions else {
        return words;
    };
    let exts: Vec<String> = exts
        .split(',')
        .map(|e| e.trim().trim_start_matches('.').to_string())
        .filter(|e| !e.is_empty())
        .collect();
    if exts.is_empty() {
        return words;
    }
    let mut out = Vec::with_capacity(words.len() * (exts.len() + 1));
    for w in words {
        out.push(w.clone());
        if !w.contains('.') && !w.ends_with('/') {
            for e in &exts {
                out.push(format!("{}.{}", w, e));
            }
        }
    }
    out
}

fn join(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}
