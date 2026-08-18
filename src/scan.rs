use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use colored::Colorize;
use serde::Serialize;

use crate::cli::ScanArgs;
use crate::client::ClientConfig;
use crate::fmt::group;

#[derive(Clone, Copy)]
struct Wildcard {
    status: u16,
    size: u64,
}

#[derive(Serialize)]
struct Found {
    url: String,
    status: u16,
    size: u64,
    ms: f64,
    title: Option<String>,
    /// Wildcard baseline for this base, never serialized
    #[serde(skip)]
    wc: Option<Wildcard>,
}

pub async fn run(args: &ScanArgs, json: bool) -> anyhow::Result<()> {
    let words = load_words_source(args)?;
    if words.is_empty() {
        anyhow::bail!("wordlist has no entries");
    }

    let config = ClientConfig::from_http(&args.http);
    let client = config.build()?;
    let bases = match &args.url {
        Some(url) => vec![url.clone()],
        None => read_stdin_urls()?,
    };

    let mut base_words: Vec<Arc<Vec<String>>> = Vec::new();
    let mut extra = 0usize;
    for base in &bases {
        let mut w = words.clone();
        if args.robots {
            let found = robots_paths(base, &client).await;
            let mut seen: HashSet<String> = w.iter().cloned().collect();
            for p in found {
                if seen.insert(p.clone()) {
                    w.push(p);
                    extra += 1;
                }
            }
        }
        base_words.push(Arc::new(w));
    }
    if extra > 0 && !json && !args.silent {
        println!("  {} paths from robots/sitemap", extra);
    }

    let workers = args.concurrency.max(1);
    let delay = Duration::from_millis(args.delay);
    let effective_depth = if args.no_recursion {
        0
    } else {
        args.depth as usize
    };
    let start = Instant::now();

    let allow: Option<Vec<u16>> = match &args.match_status {
        Some(spec) => Some(parse_status_list(spec, "--match-status")?),
        None => None,
    };
    let exclude: Vec<u16> = match &args.filter_status {
        Some(spec) => parse_status_list(spec, "--filter-status")?,
        None => Vec::new(),
    };
    let exclude_size = args.filter_size;

    let mut tried = 0u64;
    let mut found = Vec::new();
    for (base, w) in bases.iter().zip(&base_words) {
        let wc = wildcard(base, &client).await;
        let (mut f, t) = scan_base(
            base,
            w,
            &config,
            workers,
            delay,
            args.title,
            effective_depth,
            wc,
        )
        .await;
        tried += t;
        found.append(&mut f);
    }

    found.sort_by(|a, b| a.status.cmp(&b.status).then(a.url.cmp(&b.url)));
    let shown = filter_shown(&found, &allow, &exclude, exclude_size);

    if let Some(out) = args.output.as_deref() {
        if json {
            let data = serde_json::to_string_pretty(&serde_json::json!({
                "tried": tried,
                "found": shown.len(),
                "paths": shown,
            }))?;
            std::fs::write(out, data)?;
            if !args.silent {
                eprintln!("  wrote JSON to {}", out);
            }
        } else {
            let mut data = String::new();
            for f in &shown {
                data.push_str(&format!("{} {}\n", f.status, f.url));
            }
            std::fs::write(out, data)?;
            if !args.silent {
                println!("  wrote {} paths to {}", shown.len(), out);
            }
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

    if args.silent {
        for f in &shown {
            println!("{} {}", f.status, f.url);
        }
        return Ok(());
    }

    for base in &bases {
        println!();
        println!("  {} {}", "auger scan".bold().cyan(), base);
        let prefix = base.trim_end_matches('/');
        for f in &shown {
            if f.url.starts_with(prefix) {
                println!("{}", row(f));
            }
        }
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

fn read_stdin_urls() -> anyhow::Result<Vec<String>> {
    let urls = crate::cli::read_urls(std::io::stdin().lock());
    if urls.is_empty() {
        anyhow::bail!("no base URLs read from stdin");
    }
    Ok(urls)
}

/// `-w -` reads the wordlist from stdin; otherwise it is loaded from the file.
fn load_words_source(args: &ScanArgs) -> anyhow::Result<Vec<String>> {
    if args.wordlist == "-" {
        if args.stdin {
            anyhow::bail!(
                "cannot read both the base URLs and the wordlist from stdin; \
                 pass the URLs as arguments or use a wordlist file"
            );
        }
        Ok(expand(
            crate::cli::read_words(std::io::stdin().lock()),
            args.extensions.as_deref(),
        ))
    } else {
        Ok(expand(
            load_words(&args.wordlist)?,
            args.extensions.as_deref(),
        ))
    }
}

/// A catch-all response for a random path, used as a false-positive baseline.
async fn wildcard(base: &str, client: &reqwest::Client) -> Option<Wildcard> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let target = join(base, &format!("/auger_wc_{}", nonce));
    let resp = client.get(&target).send().await.ok()?;
    let status = resp.status().as_u16();
    if status == 404 {
        return None;
    }
    let body = resp.bytes().await.ok()?;
    Some(Wildcard {
        status,
        size: body.len() as u64,
    })
}

fn parse_status_list(spec: &str, flag: &str) -> anyhow::Result<Vec<u16>> {
    let list: Result<Vec<u16>, _> = spec.split(',').map(|p| p.trim().parse()).collect();
    list.map_err(|_| {
        anyhow::anyhow!(
            "{} must be comma separated status codes, e.g. 200,301,403",
            flag
        )
    })
}

/// Exclusions (wildcard match, --filter-status, --filter-size) apply first and
/// unconditionally; --match-status is an allowlist on top.
fn filter_shown<'a>(
    found: &'a [Found],
    allow: &Option<Vec<u16>>,
    exclude: &[u16],
    exclude_size: Option<u64>,
) -> Vec<&'a Found> {
    found
        .iter()
        .filter(|f| {
            let wildcard_hit =
                f.wc.is_some_and(|w| f.status == w.status && f.size == w.size);
            !wildcard_hit
                && !exclude.contains(&f.status)
                && !exclude_size.is_some_and(|s| f.size == s)
                && match allow {
                    Some(list) => list.contains(&f.status),
                    None => f.status != 404,
                }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn scan_base(
    base: &str,
    words: &Arc<Vec<String>>,
    config: &ClientConfig,
    workers: u32,
    delay: Duration,
    with_title: bool,
    depth_limit: usize,
    wc: Option<Wildcard>,
) -> (Vec<Found>, u64) {
    let mut tried = 0u64;
    let (mut found, t) = probe(base, words, config, workers, delay, with_title, wc).await;
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
            let (mut more, t) = probe(d, words, config, workers, delay, with_title, wc).await;
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
    wc: Option<Wildcard>,
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
                    let ms = t0.elapsed().as_secs_f64() * 1000.0;
                    // Chunked responses lack Content-Length, so read the body
                    // once and derive size (and title on 2xx when requested).
                    let want_body = resp.content_length().is_none()
                        || (with_title && (200..300).contains(&status));
                    let (size, title) = if want_body {
                        match resp.bytes().await {
                            Ok(b) => {
                                let title = if with_title && (200..300).contains(&status) {
                                    extract_title(&b)
                                } else {
                                    None
                                };
                                (b.len() as u64, title)
                            }
                            Err(_) => (0, None),
                        }
                    } else {
                        (resp.content_length().unwrap_or(0), None)
                    };
                    found.push(Found {
                        url: target,
                        status,
                        size,
                        ms,
                        title,
                        wc,
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
    fn scan_requires_url_or_stdin() {
        assert!(Cli::try_parse_from(["auger", "scan", "http://x", "-w", "w"]).is_ok());
        assert!(Cli::try_parse_from(["auger", "scan", "-w", "w", "--stdin"]).is_ok());
        assert!(Cli::try_parse_from(["auger", "scan", "-w", "w"]).is_err());
        assert!(Cli::try_parse_from(["auger", "scan", "http://x", "-w", "w", "--stdin"]).is_err());
    }

    #[test]
    fn filter_shown_drops_404() {
        let found: Vec<super::Found> = vec![found(200), found(404), found(301)];
        let shown = super::filter_shown(&found, &None, &[], None);
        let statuses: Vec<u16> = shown.iter().map(|f| f.status).collect();
        assert_eq!(statuses, vec![200, 301]);
    }

    #[test]
    fn filter_shown_match_status() {
        let found: Vec<super::Found> = vec![found(200), found(403)];
        let shown = super::filter_shown(&found, &Some(vec![403]), &[], None);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].status, 403);
    }

    #[test]
    fn filter_shown_hides_wildcard_match() {
        let wc = Some(super::Wildcard {
            status: 200,
            size: 100,
        });
        let found = vec![found_with(200, 100, wc), found_with(200, 42, wc)];
        let shown = super::filter_shown(&found, &None, &[], None);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].size, 42);
    }

    #[test]
    fn filter_shown_keeps_same_status_different_size() {
        let wc = Some(super::Wildcard {
            status: 200,
            size: 100,
        });
        let found = vec![found_with(200, 100, wc), found_with(200, 200, wc)];
        let shown = super::filter_shown(&found, &None, &[], None);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].size, 200);
    }

    #[test]
    fn filter_shown_exclude_status() {
        let found = vec![found(200), found(403), found(500)];
        let shown = super::filter_shown(&found, &None, &[403, 500], None);
        let statuses: Vec<u16> = shown.iter().map(|f| f.status).collect();
        assert_eq!(statuses, vec![200]);
    }

    #[test]
    fn filter_shown_exclude_size() {
        let found = vec![found_with(200, 1024, None), found_with(200, 2048, None)];
        let shown = super::filter_shown(&found, &None, &[], Some(1024));
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].size, 2048);
    }

    #[test]
    fn parse_status_list_ok() {
        assert_eq!(
            super::parse_status_list("403, 500", "--filter-status").unwrap(),
            vec![403, 500]
        );
    }

    #[test]
    fn parse_status_list_bad() {
        assert!(super::parse_status_list("4xx", "--filter-status").is_err());
    }

    #[test]
    fn read_urls_trims_and_skips() {
        use std::io::Cursor;
        assert_eq!(
            crate::cli::read_urls(Cursor::new(" https://a.com \n\nhttps://b.com\n")),
            vec!["https://a.com", "https://b.com"]
        );
        assert!(crate::cli::read_urls(Cursor::new("\n \n")).is_empty());
    }

    #[test]
    fn read_words_skips_comments() {
        use std::io::Cursor;
        assert_eq!(
            crate::cli::read_words(Cursor::new("admin\n# comment\n  private  \n")),
            vec!["admin", "private"]
        );
    }

    #[test]
    fn wordlist_dash_conflicts_with_stdin_urls() {
        let s = parse_scan(&["auger", "scan", "-w", "-", "--stdin"]);
        assert!(super::load_words_source(&s).is_err());
    }

    #[test]
    fn wordlist_dash_needs_no_positional_url() {
        // `-w -` alone is fine: URLs come from the positional argument.
        let s = parse_scan(&["auger", "scan", "http://x", "-w", "-"]);
        assert!(s.wordlist == "-");
    }

    fn found(status: u16) -> super::Found {
        found_with(status, 0, None)
    }

    fn found_with(status: u16, size: u64, wc: Option<super::Wildcard>) -> super::Found {
        super::Found {
            url: format!("http://x/{status}"),
            status,
            size,
            ms: 0.0,
            title: None,
            wc,
        }
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
            wc: None,
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
