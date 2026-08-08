use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use colored::Colorize;

use crate::cli::ScanArgs;
use crate::client::ClientConfig;
use crate::fmt::group;

struct Found {
    url: String,
    status: u16,
    size: u64,
    ms: f64,
    title: Option<String>,
}

pub async fn run(args: &ScanArgs) -> anyhow::Result<()> {
    let words = expand(load_words(&args.wordlist)?, args.extensions.as_deref());
    if words.is_empty() {
        anyhow::bail!("wordlist '{}' has no entries", args.wordlist);
    }
    let words = Arc::new(words);
    let workers = args.concurrency.max(1);
    let delay = Duration::from_millis(args.delay);
    let start = Instant::now();

    let allow: Option<Vec<u16>> = match &args.match_status {
        Some(spec) => {
            let list: Result<Vec<u16>, _> = spec.split(',').map(|p| p.trim().parse()).collect();
            Some(list.map_err(|_| {
                anyhow::anyhow!("--match-status must be comma separated status codes, e.g. 200,301,403")
            })?)
        }
        None => None,
    };

    // Fail fast on bad proxy, headers or auth before any traffic is sent.
    ClientConfig::from_http(&args.http).build()?;

    let config = ClientConfig::from_http(&args.http);
    let mut tried = 0u64;
    let (mut found, t) = probe(&args.url, &words, &config, workers, delay, args.title).await;
    tried += t;

    // Recurse into directories returned by the first pass.
    let mut seen = HashSet::new();
    let mut dirs: Vec<String> = found
        .iter()
        .filter(|f| (200..300).contains(&f.status) && f.url.ends_with('/'))
        .filter(|f| seen.insert(f.url.clone()))
        .map(|f| f.url.clone())
        .collect();
    dirs.sort();

    let mut depth = 0usize;
    while !dirs.is_empty() && depth < 3 {
        depth += 1;
        let mut next = Vec::new();
        for d in &dirs {
            let (mut more, t) = probe(d, &words, &config, workers, delay, args.title).await;
            tried += t;
            for f in &more {
                if (200..300).contains(&f.status) && f.url.ends_with('/') && seen.insert(f.url.clone()) {
                    next.push(f.url.clone());
                }
            }
            found.append(&mut more);
        }
        next.sort();
        dirs = next;
    }

    found.sort_by(|a, b| a.status.cmp(&b.status).then(a.url.cmp(&b.url)));
    let shown: Vec<&Found> = match &allow {
        Some(list) => found.iter().filter(|f| list.contains(&f.status)).collect(),
        None => found.iter().filter(|f| f.status != 404).collect(),
    };

    println!();
    println!("  {} {}", "auger scan".bold().cyan(), args.url);
    for f in &shown {
        println!("{}", row(f));
    }
    println!();
    println!(
        "  {} paths tried · {} found · {:.1}s",
        group(tried),
        group(shown.len() as u64),
        start.elapsed().as_secs_f64()
    );

    if let Some(out) = args.output.as_deref() {
        let mut data = String::new();
        for f in &shown {
            data.push_str(&format!("{} {}\n", f.status, f.url));
        }
        std::fs::write(out, data)?;
        println!("  wrote {} paths to {}", shown.len(), out);
    }
    Ok(())
}

async fn probe(
    base: &str,
    words: &Arc<Vec<String>>,
    config: &ClientConfig,
    workers: u32,
    delay: Duration,
    with_title: bool,
) -> (Vec<Found>, u64) {
    let next = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for i in 0..workers {
        let words = words.clone();
        let next = next.clone();
        let base = base.to_string();
        let config = config.clone().worker(i as usize);
        handles.push(tokio::spawn(async move {
            let client = match config.build() {
                Ok(c) => c,
                Err(_) => return (Vec::new(), 0u64),
            };
            let mut found = Vec::new();
            let mut tried = 0u64;
            loop {
                let idx = next.fetch_add(1, Ordering::Relaxed);
                if idx >= words.len() {
                    break;
                }
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                tried += 1;
                let target = join(&base, &words[idx]);
                let t0 = Instant::now();
                match client.get(&target).send().await {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        let size = resp.content_length().unwrap_or(0);
                        let ms = t0.elapsed().as_secs_f64() * 1000.0;
                        let title = if with_title && (200..300).contains(&status) {
                            read_title(resp).await
                        } else {
                            None
                        };
                        found.push(Found { url: target, status, size, ms, title });
                    }
                    Err(_) => {}
                }
            }
            (found, tried)
        }));
    }
    let mut out = Vec::new();
    let mut tried = 0u64;
    for h in handles {
        if let Ok((mut f, t)) = h.await {
            out.append(&mut f);
            tried += t;
        }
    }
    (out, tried)
}

async fn read_title(resp: reqwest::Response) -> Option<String> {
    let bytes = resp.bytes().await.ok()?;
    extract_title(&bytes)
}

fn extract_title(bytes: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(bytes);
    let lower = s.to_lowercase();
    let start = lower.find("<title")?;
    let start = lower[start..].find('>')? + start + 1;
    let end = lower[start..].find("</title>")? + start;
    let t = s[start..end].trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

fn row(f: &Found) -> String {
    let mut line = format!(
        "  {:>3} {:>9} {:>8.0}ms  {}",
        f.status,
        size_str(f.size),
        f.ms,
        f.url
    );
    if let Some(t) = &f.title {
        line.push_str(&format!("  |  {}", t));
    }
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
    let bytes = std::fs::read(path).with_context(|| format!("failed to read wordlist '{}'", path))?;
    let text = decode_wordlist(&bytes, path)?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect())
}

fn decode_wordlist(bytes: &[u8], path: &str) -> anyhow::Result<String> {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16(&units)
            .map_err(|_| anyhow::anyhow!("wordlist '{}' has invalid UTF-16 LE data", path));
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16(&units)
            .map_err(|_| anyhow::anyhow!("wordlist '{}' has invalid UTF-16 BE data", path));
    }
    let text = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        bytes
    };
    match std::str::from_utf8(text) {
        Ok(s) => Ok(s.to_string()),
        Err(_) => Err(anyhow::anyhow!(
            "wordlist '{}' is not valid UTF-8 — if created with PowerShell, re-save it as UTF-8: `Get-Content {} | Set-Content {} -Encoding utf8`",
            path, path, path
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::decode_wordlist;

    #[test]
    fn plain_utf8() {
        assert_eq!(decode_wordlist(b"admin\nprivate\n", "w").unwrap(), "admin\nprivate\n");
    }

    #[test]
    fn utf8_with_bom() {
        assert_eq!(decode_wordlist(b"\xef\xbb\xbfadmin", "w").unwrap(), "admin");
    }

    #[test]
    fn utf16_le_with_bom() {
        let text = "admin\r\nprivate";
        let mut b = vec![0xFF, 0xFE];
        for u in text.encode_utf16() {
            b.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_wordlist(&b, "w").unwrap(), text);
    }

    #[test]
    fn utf16_be_with_bom() {
        let text = "admin\nprivate";
        let mut b = vec![0xFE, 0xFF];
        for u in text.encode_utf16() {
            b.extend_from_slice(&u.to_be_bytes());
        }
        assert_eq!(decode_wordlist(&b, "w").unwrap(), text);
    }

    #[test]
    fn invalid_utf8_mentions_file() {
        let err = decode_wordlist(&[0x61, 0xC3], "words.txt").unwrap_err().to_string();
        assert!(err.contains("words.txt"));
        assert!(err.contains("not valid UTF-8"));
    }

    #[test]
    fn invalid_utf16_le_data() {
        let err = decode_wordlist(&[0xFF, 0xFE, 0x00, 0xD8], "words.txt")
            .unwrap_err()
            .to_string();
        assert!(err.contains("UTF-16"));
    }

    #[test]
    fn title_extracted() {
        assert_eq!(
            super::extract_title(b"<html><head><title>Example Domain</title></head></html>"),
            Some("Example Domain".to_string())
        );
    }

    #[test]
    fn title_missing() {
        assert_eq!(super::extract_title(b"<h1>no title here</h1>"), None);
    }

    #[test]
    fn title_empty() {
        assert_eq!(super::extract_title(b"<title></title>"), None);
    }
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
