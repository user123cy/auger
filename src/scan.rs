use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

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
