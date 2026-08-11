# Changelog

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
