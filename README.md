# auger

![crates.io](https://img.shields.io/crates/v/auger.svg)
![downloads](https://img.shields.io/crates/d/auger.svg)
![License](https://img.shields.io/badge/license-MIT-blue)
![CI](https://github.com/user123cy/auger/actions/workflows/ci.yml/badge.svg)

Load test, discover and inspect HTTP endpoints from the terminal.

Twelve commands, one binary:

- `auger run` — hammer a URL with concurrent requests, get percentiles, a latency histogram and a flamegraph
- `auger scan` — brute-force endpoints with a wordlist
- `auger check` — inspect status, HTTP version and security headers with an A-F grade
- `auger cert` — show the TLS certificate for a host
- `auger ping` — measure per-phase latency (DNS/TCP/TLS/TTFB) of a single request
- `auger tech` — detect technologies, frameworks and CMS from headers and HTML
- `auger cors` — test CORS misconfigurations and cross-origin vulnerabilities
- `auger dns` — enumerate DNS records, check security, and discover subdomains
- `auger fuzz` — fuzz HTTP endpoints with path traversal, XSS, SSRF and more
- `auger chaos` — chaos engineering: inject failures and measure resilience score
- `auger fingerprint` — behavioral fingerprinting: create unique server signature
- `auger story` — generate human-readable narrative from load test reports

## install

From crates.io:

```
cargo install auger
```

Or the install script (downloads the latest prebuilt binary):

```
curl -fsSL https://raw.githubusercontent.com/user123cy/auger/main/scripts/install.sh | bash
```

Prebuilt binaries for Linux, macOS and Windows are attached to every [release](https://github.com/user123cy/auger/releases).

## load test

```
auger run https://api.example.com/                  # 20 workers, 5s
auger run https://api.example.com/ -c 200 -d 30s    # 200 workers, 30s
auger run https://api.example.com/ -m POST -d 10s
auger run https://api.example.com/ --random-ua -H "X-Token: abc"
auger run https://api.example.com/ -m POST --body '{"user":1}'   # inline request body
auger run https://api.example.com/ -d 30s --quiet                # no progress line
auger run https://api.example.com/ -d 30s --json                 # machine-readable output
auger run https://api.example.com/ -d 30s --tui                  # live dashboard (requires --features tui)
auger run https://api.example.com/ -d 30s --warmup 3s            # discard the first 3s from the stats
auger run https://api.example.com/ -d 30s --max-errors 50        # stop early after 50 errors
auger run https://api.example.com/ -d 30s --status-ok 2xx,3xx    # count other statuses as errors
auger run https://a.example.com/ https://b.example.com/ -d 10s   # battle: compare & crown a winner
auger run https://a.example.com/ https://b.example.com/ --markdown  # battle as a markdown table
auger run -d 10s --urls-file urls.txt                             # battle from a file, one URL per line
cat urls.txt | auger run -d 10s --stdin                            # battle from stdin
auger run https://api.example.com/ -d 10s --markdown             # report as markdown (paste in a PR)
auger run https://api.example.com/ -d 10s --webhook https://discord.com/api/webhooks/...  # post a summary
```

`--status-ok` decides which responses count as success (exact codes like `401` or classes like `2xx`). Responses outside the list are counted as errors and set the exit code to 1, so a run against a server that answers 500 fails in CI. With `--warmup` the first seconds of the run are spent warming connections and are excluded from the statistics; the summary shows how many requests fell into the warmup window (`· 12 in warmup`) so the count reconciles with the live progress counter. `--max-errors` aborts the run as soon as that many errors accumulate.

Pass several URLs to `run` to compare endpoints side by side: auger prints a comparison matrix and crowns a winner (fastest p50 among healthy endpoints — ones that answered requests without errors). URL lists can come from a file or stdin instead of the command line — `--urls-file urls.txt` and `--stdin` combine with any positional URLs, so a single command can load test dozens of endpoints in one battle. The exit code is 1 when any endpoint reports errors. `--markdown` renders the report — or the battle matrix — as a table you can paste straight into a PR comment, and `--webhook` posts a one-line summary to a Discord (`content`) or Slack (`text`) webhook when the run finishes; a failing webhook only warns on stderr, it never fails the run. Every text report ends with an `insights` section that reads the numbers for you: tail-latency ratio (p99 vs p50), error rate and 4xx/5xx share.

The `--tui` dashboard shows live req/s, latency percentiles, a histogram, a req/s sparkline, status codes and errors, with a progress bar. `p` pauses the run, `q`/`Esc` quits.
```
auger completions bash   # generate a bash completion script (also zsh, fish, powershell, elvish)
```

Save a baseline and compare against it later:

```
auger run https://api.example.com/ -d 30s -s baseline.json
auger run https://api.example.com/ -d 30s --compare baseline.json --threshold 1.2
```

Any percentile slower than the baseline by more than the threshold flags a regression. A regression sets the exit code to 1 — hook it into CI. `--json` prints the diff rows instead of the table.

## discover

```
auger scan https://example.com/ -w wordlist.txt               # find non-404 paths
auger scan https://example.com/ -w wordlist.txt -e php,html   # also try .php and .html
auger scan https://example.com/ -w wordlist.txt -o hits.txt   # save status + url lines
auger scan https://example.com/ -w wordlist.txt -R            # also probe robots.txt + sitemap.xml paths
auger scan https://example.com/ -w wordlist.txt --depth 2     # cap recursion into 2xx directories
auger scan https://example.com/ -w wordlist.txt --no-recursion # probe only the base path
auger scan https://example.com/ -w wordlist.txt --json        # machine-readable output
cat urls.txt | auger scan -w wordlist.txt --stdin             # scan many bases from stdin
cat urls.txt | auger scan -w wordlist.txt --stdin --silent    # print only "status url" lines
auger scan https://example.com/ -w wordlist.txt --filter-status 403,500  # drop these statuses
auger scan https://example.com/ -w wordlist.txt --filter-size 1234       # drop responses of this exact size
cat words.txt | auger scan https://example.com/ -w - --json -o hits.json  # wordlist from stdin, JSON to a file
```

`-w -` reads the wordlist from stdin (`cat words.txt | auger scan https://example.com/ -w -`). With `--json`, the `-o` file holds the machine-readable result (`tried`, `found`, `paths`) instead of `status url` lines, while stdout still gets the JSON.

Before scanning each base, auger probes a random path to learn what a catch-all response looks like (status + body size) and hides matching hits — so a SPA that answers 200 to every path won't flood the results. Filters (`--filter-status`, `--filter-size`) apply before `--match-status`.

## latency

```
auger ping https://example.com/          # DNS, TCP, TLS, TTFB, total for one request
auger ping example.com -c 3              # 3 attempts + min/avg/max per phase
auger ping https://example.com/ --json   # machine-readable attempt object
auger ping http://localhost:8080/api     # missing scheme defaults to http
```

Useful for spotting which phase of a connection is slow — a bad DNS resolver, a slow TLS handshake or a server that takes long to send headers.

## inspect

```
auger check https://example.com/
auger check https://example.com/ --json
```

Shows status, HTTP version, server and which security headers are present: HSTS, CSP, clickjacking, mime sniffing, referrer, permissions, COOP, CORP. The headers are weighted and scored into a letter grade (A-F) in the style of securityheaders.com. Missing headers show the exact header line to add. Cookies are listed with their Secure/HttpOnly/SameSite flags. For HTTPS sites the TLS line reports the certificate issuer, expiry date and days left, with a warning when it expires within 30 days or has already expired.

## certificate

```
auger cert example.com
auger cert example.com:8443
auger cert https://example.com/ --json
```

Shows the subject, issuer, TLS version, key size, signature algorithm, chain length, validity dates, days left and SANs. Exits non-zero when the certificate is expired and warns on stderr when it expires within 30 days.

## tech detection

```
auger tech https://example.com/                # detect frameworks, CMS, servers
auger tech https://example.com/ --json         # machine-readable output
auger tech https://example.com/ -f urls.txt    # scan multiple URLs from a file
```

Identifies technologies from HTTP headers, cookies, meta tags and HTML patterns. Detects web servers (Nginx, Apache, IIS), frameworks (React, Vue, Angular, Next.js, Rails, Laravel), CMS (WordPress, Drupal, Joomla), CDNs (Cloudflare, Fastly), hosting providers (Vercel, Netlify), analytics, and more. WordPress detection includes plugin enumeration. Each technology includes a confidence level (certain/likely/possible).

## CORS testing

```
auger cors https://example.com/api             # test for CORS misconfigurations
auger cors https://a.com https://b.com          # test multiple URLs
auger cors https://example.com/ --json          # machine-readable output
```

Probes how the server responds to various Origin headers (evil.com, null, subdomains, encoded bypasses). Reports risk levels (critical/high/medium/low) for misconfigurations that could allow cross-origin data theft. Exits 1 when a vulnerability is found — hook it into security CI.

## DNS enumeration

```
auger dns example.com                          # A records
auger dns example.com --all                     # all record types (A, MX, TXT, NS, SOA...)
auger dns example.com -t MX,TXT,NS              # specific record types
auger dns example.com --subdomains              # quick subdomain check (80 common names)
auger dns example.com --wordlist subs.txt       # subdomain brute-force from file
auger dns example.com --json                    # machine-readable output
```

Queries DNS-over-HTTPS (Google) for record enumeration. Checks security issues: weak SPF records (+all, ~all), missing DMARC, missing MX. Subdomain enumeration resolves common prefixes (www, mail, api, admin, etc.) or a custom wordlist.

## fuzzing

```
auger fuzz https://example.com/                            # built-in payloads (path traversal, XSS, SQLi...)
auger fuzz https://example.com/ --wordlist payloads.txt    # custom payload list
auger fuzz https://example.com/ --builtin                   # explicit built-in payloads
auger fuzz https://example.com/ -i query                    # inject into query string
auger fuzz https://example.com/ -i body -m POST --body '{"user":"FUZZ"}'  # inject into body
auger fuzz https://example.com/ -i subdomain                # test subdomains
auger fuzz https://example.com/ -i wordlist                 # replace entire path
auger fuzz https://example.com/ --max-payloads 100          # limit number of payloads
auger fuzz https://example.com/ --json                      # machine-readable output
```

Fuzzes HTTP endpoints with configurable injection points. Built-in payloads cover path traversal (../../../etc/passwd), XSS, SQL injection, SSRF (169.254.169.254), open redirects, CRLF injection, Log4j, NoSQL injection, and sensitive path discovery (.env, .git, actuator, phpinfo). Reports interesting responses (2xx on sensitive paths, 3xx redirects, 5xx errors). Exits 1 when interesting responses are found.

## chaos engineering

```
auger chaos https://api.example.com/           # 5 rounds per phase, measure resilience
auger chaos https://api.example.com/ -r 100    # 100 rounds per phase
auger chaos https://api.example.com/ --json    # machine-readable output
```

Runs 5 phases of chaos against the server: normal baseline, delayed requests (slow clients), malformed requests (bad HTTP), partial requests (dropped connections), and oversized requests (1MB body). Scores resilience from 0-100 based on how well the server handles each scenario. Reports issues like "server crashes on malformed input" or "doesn't handle disconnects". Grade A-F. Exit code 1 when score < 60.

## fingerprinting

```
auger fingerprint https://example.com/         # analyze server behavior
auger fingerprint https://example.com/ --json  # machine-readable output
```

Creates a unique behavioral fingerprint of an HTTP server by analyzing: header ordering, response timing patterns, error behavior, method support, and security posture. Detects anomalies like "error responses are 10x slower" (heavy logging), "TRACE method enabled" (XST vulnerability), or "X-Forwarded-For reflected" (proxy detected). Generates a deterministic fingerprint hash that changes when the server's behavior changes — useful for detecting infrastructure changes or drift.

## narrative reports

```
auger story report.json                        # generate narrative from a load test
auger story report.json --json                 # machine-readable output
```

Tells the story of what happened during a load test in plain English. Like: "At 0:15, errors begin to appear — 42 out of 10,000 requests failed. The first error was 'connection reset'. The server returned 503 for 23% of requests..." Ends with a summary, key findings, and actionable recommendations. Perfect for sharing results with non-technical stakeholders.

## reports

```
auger run https://api.example.com/ -d 10s -s report.json
auger report report.json            # print it again later
auger report report.json --markdown # as a markdown table
auger html report.json -o r.html    # self-contained HTML with a chart
auger compare before.json after.json   # exits 1 when the "after" report regresses
```

## example

```
  auger https://api.example.com/
  20 workers · 5s · 12,403 req · 2,481 req/s
  0 errors · 12.4 MB downloaded

  percentiles (ms)
   p50   12
   p75   18
   p90   26
   p95   34
   p99   52
   max  190

  latency histogram (ms)
     1–5   ██████████  1040
     5–9   ██████████  1078
    9–13   ██████████  1111
   13–26   █████████  976
   26–52   ██████  621
   52–190  █████  530
```

## exit codes

`auger run` exits 0 when the run finished without errors, 1 when any error occurred — including responses outside `--status-ok`, a `--max-errors` abort, or a regression against a saved baseline via `--compare`. `auger compare` also exits 1 on regression. That makes the commands safe to gate CI on.

On Windows PowerShell use `$LASTEXITCODE` to read the exit code — `$?` is a boolean there and prints `False` for a non-zero exit:

```
PS> .\auger.exe run https://api.example.com/ -d 30s --status-ok 2xx
PS> $LASTEXITCODE
1
```

## why

Most load testers are huge — a config file, a GUI or a whole platform just to measure a couple of endpoints. auger is one binary, a few flags and a fast release build.

## license

MIT
