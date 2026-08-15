# auger

![crates.io](https://img.shields.io/crates/v/auger.svg)
![downloads](https://img.shields.io/crates/d/auger.svg)
![License](https://img.shields.io/badge/license-MIT-blue)
![CI](https://github.com/user123cy/auger/actions/workflows/ci.yml/badge.svg)

Load test, discover and inspect HTTP endpoints from the terminal.

Five commands, one binary:

- `auger run` — hammer a URL with concurrent requests, get percentiles, a latency histogram and a flamegraph
- `auger scan` — brute-force endpoints with a wordlist
- `auger check` — inspect status, HTTP version and security headers with an A-F grade
- `auger cert` — show the TLS certificate for a host
- `auger ping` — measure per-phase latency (DNS/TCP/TLS/TTFB) of a single request

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
auger run https://api.example.com/ -d 10s --markdown             # report as markdown (paste in a PR)
auger run https://api.example.com/ -d 10s --webhook https://discord.com/api/webhooks/...  # post a summary
```

`--status-ok` decides which responses count as success (exact codes like `401` or classes like `2xx`). Responses outside the list are counted as errors and set the exit code to 1, so a run against a server that answers 500 fails in CI. With `--warmup` the first seconds of the run are spent warming connections and are excluded from the statistics; the summary shows how many requests fell into the warmup window (`· 12 in warmup`) so the count reconciles with the live progress counter. `--max-errors` aborts the run as soon as that many errors accumulate.

Pass several URLs to `run` to compare endpoints side by side: auger prints a comparison matrix and crowns a winner (fastest p50 among healthy endpoints — ones that answered requests without errors). The exit code is 1 when any endpoint reports errors. `--markdown` renders the report — or the battle matrix — as a table you can paste straight into a PR comment, and `--webhook` posts a one-line summary to a Discord (`content`) or Slack (`text`) webhook when the run finishes; a failing webhook only warns on stderr, it never fails the run. Every text report ends with an `insights` section that reads the numbers for you: tail-latency ratio (p99 vs p50), error rate and 4xx/5xx share.

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
```

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

Shows status, HTTP version, server and which security headers are present: HSTS, CSP, clickjacking, mime sniffing, referrer, permissions, COOP, CORP. The headers are weighted and scored into a letter grade (A-F) in the style of securityheaders.com. Missing headers show the exact header line to add. Cookies are listed with their Secure/HttpOnly/SameSite flags.

## certificate

```
auger cert example.com
auger cert example.com:8443
auger cert https://example.com/ --json
```

Shows the subject, issuer, TLS version, key size, signature algorithm, chain length, validity dates, days left and SANs. Exits non-zero when the certificate is expired and warns on stderr when it expires within 30 days.

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
