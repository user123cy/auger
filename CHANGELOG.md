# Changelog

## 0.4.0

### Added
- `auger run --tui` — live dashboard: latency histogram, percentiles, req/s sparkline, status codes and error breakdown, plus a progress gauge. `p` pauses the run (workers stop sending, wall clock keeps running), `q`/`Esc` quits. The final report prints normally after the run. Build with `cargo install auger --features tui`.
- `auger run --status-ok` — list of status codes or classes counted as success, default `2xx,3xx`. Responses outside the list count as errors and make the exit code 1, so CI can rely on it. Exact codes (`401`) and classes (`2xx`) are both accepted.
- `auger run --warmup` — discard the first seconds of the run from the statistics (latency, ttfb, statuses), keeping connection errors visible. The summary shows how many requests fell in the warmup window (`· 12 in warmup`) so the count reconciles with the live progress counter.
- `auger run --max-errors` — abort the run as soon as this many errors accumulate.
- TTFB now reports full percentiles (p50–p99 and max) in the text report and markdown table.
- `auger run <url1> <url2> …` — battle mode: hammer several endpoints in one command and get a side-by-side comparison matrix plus a winner (fastest p50 among healthy endpoints; a dead or erroring endpoint never wins). Exit code 1 when any endpoint errors.
- `auger run --webhook <url>` — post a one-line summary to a Discord or Slack webhook when the run finishes (Discord `content` and Slack `text` payloads auto-detected). A failing webhook only warns on stderr.
- `auger run --markdown` — print the report (or the battle matrix) as a markdown table, ready to paste into a PR comment.
- Text reports end with an `insights` section — plain-language reading of tail-latency ratio (p99 vs p50), error rate and 4xx/5xx share.
- crates.io page polish: `readme`, `homepage`, `documentation`, `repository`, `keywords` and `categories` metadata.
- `auger completions <shell>` — generate completion scripts for bash, zsh, fish, powershell or elvish.
- CI now runs clippy, tests and the release build with the `tui` feature enabled.
- Integration test: `auger run --tui` against a local server exits on its own after the duration with the report printed.

### Fixed
- The TUI no longer hangs after the load test finishes: it runs on its own thread and is told to stop when the run ends, always restoring raw mode and the alternate screen.
- `Report::new` is gated behind the `tui` feature, keeping the default build clippy-clean.

### JSON changes (pre-1.0)
- `auger run` reports gain `errors_status`: responses whose status is not in `--status-ok`.
- `auger run` reports gain `warmup_discarded`: requests sent during `--warmup` and excluded from the statistics.

## 0.3.0

### Added
- `auger ping` — measure per-phase latency of a single HTTP request: DNS, TCP, TLS, TTFB and total, plus status, server, body size, HTTP version and certificate details. `-c/--count` sends multiple attempts and prints min/avg/max; `--json` wraps attempts in a machine-readable object. Missing scheme defaults to http.
- `auger scan --filter-status` — exclude comma separated status codes, e.g. `--filter-status 403,500`.
- `auger scan --filter-size` — exclude responses with exactly this body size in bytes.
- `auger scan` wildcard detection — probes a random path per base URL before scanning and hides responses matching that status+size, cutting SPA/catch-all false positives. Exclusions apply first; `--match-status` filters on top.
- `auger check` — cookies now report Secure/HttpOnly/SameSite flags.
- `auger run --compare` / `auger compare` — exit code 1 on regression (CI-friendly) and `--json` prints the diff rows.

### Changed
- Shared TLS handshake module (`tls.rs`) used by `cert`, `check` and `ping` — one implementation instead of two copies.
- Default User-Agent now reports the real version (`auger/0.3.0`) instead of a hardcoded `auger/0.1`.
- Report summary renders the run duration with one decimal.

### JSON changes (pre-1.0)
- `auger check` includes `cookies` with `name`, `same_site`, `secure`, `http_only`.
- `auger run --compare --json` and `auger compare --json` print an array of diff rows (label, before, after, regression, improved) instead of the table.
