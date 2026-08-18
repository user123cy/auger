use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::thread;

/// Always answers 200 — used for load tests and battles.
fn spawn_ok_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            thread::spawn(move || {
                let mut buf = [0u8; 4096];
                if stream.read(&mut buf).is_err() {
                    return;
                }
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                );
                let _ = stream.flush();
            });
        }
    });
    format!("http://{addr}/")
}

/// Answers 200 for paths containing `known`, 404 otherwise — for scans.
fn spawn_scan_server(known: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let known = known.to_string();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let known = known.clone();
            thread::spawn(move || {
                let mut buf = [0u8; 4096];
                if stream.read(&mut buf).is_err() {
                    return;
                }
                let req = String::from_utf8_lossy(&buf);
                let path = req.split_whitespace().nth(1).unwrap_or("/");
                let (status, body) = if path.contains(&known) {
                    ("200 OK", "found")
                } else {
                    ("404 Not Found", "nope")
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            });
        }
    });
    format!("http://{addr}/")
}

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("auger_{}_{}.txt", name, std::process::id()))
}

#[test]
fn battle_from_urls_file() {
    let a = spawn_ok_server();
    let b = spawn_ok_server();
    let path = temp_path("battle");
    std::fs::write(&path, format!("{a}\n{b}\n")).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_auger"))
        .args([
            "run",
            "--urls-file",
            path.to_str().unwrap(),
            "-d",
            "1s",
            "-c",
            "2",
            "--quiet",
        ])
        .output()
        .unwrap();

    let _ = std::fs::remove_file(&path);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("battle"), "stdout: {stdout}");
    assert!(stdout.contains("winner"), "stdout: {stdout}");
    assert!(stdout.contains(&a), "stdout: {stdout}");
    assert!(stdout.contains(&b), "stdout: {stdout}");
}

#[test]
fn battle_from_stdin() {
    let a = spawn_ok_server();
    let b = spawn_ok_server();
    let mut child = Command::new(env!("CARGO_BIN_EXE_auger"))
        .args(["run", "--stdin", "-d", "1s", "-c", "2", "--quiet"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{a}\n{b}\n").as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("winner"), "stdout: {stdout}");
}

#[test]
fn single_url_from_file_runs_normally() {
    let a = spawn_ok_server();
    let path = temp_path("single");
    std::fs::write(&path, format!("{a}\n")).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_auger"))
        .args([
            "run",
            "--urls-file",
            path.to_str().unwrap(),
            "-d",
            "1s",
            "-c",
            "2",
            "--quiet",
        ])
        .output()
        .unwrap();

    let _ = std::fs::remove_file(&path);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("percentiles"), "stdout: {stdout}");
    assert!(!stdout.contains("battle"), "stdout: {stdout}");
}

#[test]
fn scan_wordlist_from_stdin_writes_json_output() {
    let server = spawn_scan_server("admin");
    let out_path = temp_path("scan");
    let mut child = Command::new(env!("CARGO_BIN_EXE_auger"))
        .args([
            "scan",
            &server,
            "-w",
            "-",
            "-o",
            out_path.to_str().unwrap(),
            "--json",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"admin\nprivate\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"found\": 1"), "stdout: {stdout}");

    let data = std::fs::read_to_string(&out_path).unwrap();
    let _ = std::fs::remove_file(&out_path);
    assert!(data.contains("\"paths\""), "file: {data}");
    assert!(data.contains("admin"), "file: {data}");
    assert!(!data.contains("private"), "file: {data}");
}

#[test]
fn scan_wordlist_and_urls_from_stdin_conflict() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_auger"))
        .args(["scan", "-w", "-", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"http://localhost:1\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();

    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("wordlist from stdin"), "stderr: {stderr}");
}
