#![cfg(feature = "tui")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Minimal HTTP server: one thread per connection, responds 200 with a small body.
fn spawn_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                continue;
            };
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

fn wait_or_kill(mut child: Child, secs: u64) -> std::process::ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if start.elapsed() > Duration::from_secs(secs) {
            let _ = child.kill();
            panic!("auger run --tui did not exit within {secs}s (hang regression)");
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn tui_run_exits_cleanly_after_duration() {
    let url = spawn_server();
    let mut child = Command::new(env!("CARGO_BIN_EXE_auger"))
        .args(["run", "--tui", &url, "-d", "2s", "-c", "5"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out_t = thread::spawn(move || {
        let mut s = String::new();
        stdout.read_to_string(&mut s).unwrap();
        s
    });
    let err_t = thread::spawn(move || {
        let mut s = String::new();
        stderr.read_to_string(&mut s).unwrap();
        s
    });

    let status = wait_or_kill(child, 20);
    let out = out_t.join().unwrap();
    let err = err_t.join().unwrap();

    assert!(
        status.success(),
        "auger exited with {status}\nstderr: {err}\nstdout: {out}"
    );
    assert!(
        out.contains("percentiles"),
        "final report not printed after the TUI run\nstdout: {out}\nstderr: {err}"
    );
}
