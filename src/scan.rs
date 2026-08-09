use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context;
use colored::Colorize;
use serde::Serialize;

use crate::cli::ScanArgs;
use crate::client::ClientConfig;
use crate::fmt::group;

#[derive(Serialize)]
struct Found {
    url: String,
    status: u16,
    size: u64,
    ms: f64,
    title: Option<String>,
}

pub async fn run(args: &ScanArgs, json: bool) -> anyhow::Result<()> {
    let mut words = expand(load_words(&args.wordlist)?, args.extensions.as_deref());
    if words.is_empty() {
        anyhow::bail!("wordlist '{}' has no entries", args.wordlist);
    }

    let config = ClientConfig::from_http(&args.http);
    let client = config.build()?;
    let mut extra = 0usize;
    if args.robots {
        let found = robots_paths(&args.url, &client).await;
        let mut seen: HashSet<String> = words.iter().cloned().collect();
        for p in found {
            if seen.insert(p.clone()) {
                words.push(p);
                extra += 1;
            }
        }
    }
    if extra > 0 && !json {
        println!("  {} paths from robots/sitemap", extra);
    }
    let words = Arc::new(words);
    let workers = args.concurrency.max(1);
    let delay = Duration::from_millis(args.delay);
    let effective_depth = if args.no_recursion {
        0
    } else {
        args.depth as usize
    };
    let start = Instant::now();

    let allow: Option<Vec<u16>> = match &args.match_status {
        Some(spec) => {
            let list: Result<Vec<u16>, _> = spec.split(',').map(|p| p.trim().parse()).collect();
            Some(list.map_err(|_| {
                anyhow::anyhow!(
                    "--match-status must be comma separated status codes, e.g. 200,301,403"
                )
            })?)
        }
        None => None,
    };

    let mut tried = 0u64;
    let (mut found, t) = scan_base(
        &args.url,
        &words,
        &config,
        workers,
        delay,
        args.title,
        effective_depth,
    )
    .await;
    tried += t;

    found.sort_by(|a, b| a.status.cmp(&b.status).then(a.url.cmp(&b.url)));
    let shown: Vec<&Found> = match &allow {
        Some(list) => found.iter().filter(|f| list.contains(&f.status)).collect(),
        None => found.iter().filter(|f| f.status != 404).collect(),
    };

    if let Some(out) = args.output.as_deref() {
        let mut data = String::new();
        for f in &shown {
            data.push_str(&format!("{} {}\n", f.status, f.url));
        }
        std::fs::write(out, data)?;
        if !json {
            println!("  wrote {} paths to {}", shown.len(), out);
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "tried": tried,
                "found": shown.len(),
                "paths": shown,
            }))?
        );
        return Ok(());
    }

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
    Ok(())
}

async fn scan_base(
    base: &str,
    words: &Arc<Vec<String>>,
    config: &ClientConfig,
    workers: u32,
    delay: Duration,
    with_title: bool,
    depth_limit: usize,
) -> (Vec<Found>, u64) {
    let mut tried = 0u64;
    let (mut found, t) = probe(base, words, config, workers, delay, with_title).await;
    tried += t;

    let mut seen = HashSet::new();
    let mut dirs: Vec<String> = found
        .iter()
        .filter(|f| is_dir(f, &mut seen))
        .map(|f| f.url.clone())
        .collect();
    dirs.sort();

    let mut depth = 0usize;
    while !dirs.is_empty() && depth < depth_limit {
        depth += 1;
        let mut next = Vec::new();
        for d in &dirs {
            let (mut more, t) = probe(d, words, config, workers, delay, with_title).await;
            tried += t;
            for f in &more {
                if is_dir(f, &mut seen) {
                    next.push(f.url.clone());
                }
            }
            found.append(&mut more);
        }
        next.sort();
        dirs = next;
    }
    (found, tried)
}

fn is_dir(f: &Found, seen: &mut HashSet<String>) -> bool {
    (200..300).contains(&f.status) && f.url.ends_with('/') && seen.insert(f.url.clone())
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
                if let Ok(resp) = client.get(&target).send().await {
                    let status = resp.status().as_u16();
                    let size = resp.content_length().unwrap_or(0);
                    let ms = t0.elapsed().as_secs_f64() * 1000.0;
                    let title = if with_title && (200..300).contains(&status) {
                        read_title(resp).await
                    } else {
                        None
                    };
                    found.push(Found {
                        url: target,
                        status,
                        size,
                        ms,
                        title,
                    });
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
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read wordlist '{}'", path))?;
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
            path,
            path,
            path
        )),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{decode_wordlist, is_dir};
    use crate::cli::Cli;

    fn parse_scan(args: &[&str]) -> crate::cli::ScanArgs {
        match Cli::try_parse_from(args).unwrap().command {
            crate::cli::Commands::Scan(s) => s,
            _ => unreachable!(),
        }
    }

    #[test]
    fn scan_depth_default() {
        let s = parse_scan(&["auger", "scan", "http://x", "-w", "w"]);
        assert_eq!(s.depth, 3);
        assert!(!s.no_recursion);
    }

    #[test]
    fn scan_depth_custom() {
        let s = parse_scan(&["auger", "scan", "http://x", "-w", "w", "--depth", "5"]);
        assert_eq!(s.depth, 5);
    }

    #[test]
    fn scan_no_recursion_wins_over_depth() {
        let s = parse_scan(&[
            "auger",
            "scan",
            "http://x",
            "-w",
            "w",
            "--no-recursion",
            "--depth",
            "5",
        ]);
        assert!(s.no_recursion);
        assert_eq!(s.depth, 5);
    }

    #[test]
    fn dir_collected_once() {
        let mut seen = std::collections::HashSet::new();
        let f = |url: &str| super::Found {
            url: url.into(),
            status: 200,
            size: 0,
            ms: 0.0,
            title: None,
        };
        assert!(is_dir(&f("http://x/a/"), &mut seen));
        assert!(!is_dir(&f("http://x/a/"), &mut seen));
        assert!(is_dir(&f("http://x/b/"), &mut seen));
        assert!(!is_dir(&f("http://x/b"), &mut seen));
    }

    #[test]
    fn plain_utf8() {
        assert_eq!(
            decode_wordlist(b"admin\nprivate\n", "w").unwrap(),
            "admin\nprivate\n"
        );
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
        let err = decode_wordlist(&[0x61, 0xC3], "words.txt")
            .unwrap_err()
            .to_string();
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

async fn robots_paths(base: &str, client: &reqwest::Client) -> Vec<String> {
    let mut out = Vec::new();

    if let Ok(resp) = client.get(join(base, "robots.txt")).send().await
        && resp.status().is_success()
        && let Ok(text) = resp.text().await
    {
        for line in text.lines() {
            let lower = line.to_lowercase();
            if lower.starts_with("allow:") || lower.starts_with("disallow:") {
                let value = line.split_once(':').map(|(_, v)| v.trim()).unwrap_or("");
                if let Some(p) = clean_path(value) {
                    out.push(p);
                }
            }
        }
    }

    if let Ok(resp) = client.get(join(base, "sitemap.xml")).send().await
        && resp.status().is_success()
        && let Ok(text) = resp.text().await
    {
        for loc in extract_locs(&text) {
            let p = path_of(&loc);
            if p.len() > 1 {
                out.push(p);
            }
        }
    }
    out
}

fn clean_path(raw: &str) -> Option<String> {
    let p = raw
        .replace('*', "")
        .trim_end_matches('$')
        .trim()
        .to_string();
    if p.len() <= 1 || !p.starts_with('/') {
        return None;
    }
    Some(p)
}

fn extract_locs(xml: &str) -> Vec<String> {
    let lower = xml.to_lowercase();
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(rel) = lower[pos..].find("<loc>") {
        let start = pos + rel + 5;
        let Some(end) = lower[start..].find("</loc>") else {
            break;
        };
        let loc = xml[start..start + end].trim();
        if !loc.is_empty() {
            out.push(loc.to_string());
        }
        pos = start + end + 6;
    }
    out
}

fn path_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let path = rest.split_once('/').map(|(_, p)| p).unwrap_or("");
    if path.is_empty() {
        "/".into()
    } else {
        format!("/{}", path)
    }
}

#[cfg(test)]
mod path_tests {
    use super::{clean_path, extract_locs, path_of};

    #[test]
    fn clean_path_strips_wildcards() {
        assert_eq!(clean_path("/admin/*"), Some("/admin/".to_string()));
        assert_eq!(clean_path("/private$"), Some("/private".to_string()));
        assert_eq!(clean_path("admin"), None);
        assert_eq!(clean_path("/"), None);
    }

    #[test]
    fn extract_locs_parses_sitemap() {
        let xml = "<urlset><url><loc>https://a.com/one</loc></url>\
                   <url><loc>https://a.com/two</loc></url></urlset>";
        assert_eq!(
            extract_locs(xml),
            vec!["https://a.com/one", "https://a.com/two"]
        );
    }

    #[test]
    fn extract_locs_empty() {
        assert_eq!(extract_locs("<urlset></urlset>"), Vec::<String>::new());
    }

    #[test]
    fn path_of_extracts_path() {
        assert_eq!(path_of("https://a.com/x/y?z=1"), "/x/y?z=1");
        assert_eq!(path_of("https://a.com/"), "/");
        assert_eq!(path_of("/foo"), "/foo");
    }
}
