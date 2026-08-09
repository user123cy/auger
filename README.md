# auger

![crates.io](https://img.shields.io/crates/v/auger.svg)
![downloads](https://img.shields.io/crates/d/auger.svg)
![License](https://img.shields.io/badge/license-MIT-blue)
![CI](https://github.com/user123cy/auger/actions/workflows/ci.yml/badge.svg)

Load test, discover and inspect HTTP endpoints from the terminal.

Four commands, one binary:

- `auger run` — hammer a URL with concurrent requests, get percentiles, a latency histogram and a flamegraph
- `auger scan` — brute-force endpoints with a wordlist
- `auger check` — inspect status, HTTP version and security headers with an A-F grade
- `auger cert` — show the TLS certificate for a host

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
```

Save a baseline and compare against it later:

```
auger run https://api.example.com/ -d 30s -s baseline.json
auger run https://api.example.com/ -d 30s --compare baseline.json --threshold 1.2
```

Any percentile slower than the baseline by more than the threshold flags a regression.

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
```

## inspect

```
auger check https://example.com/
auger check https://example.com/ --json
```

Shows status, HTTP version, server and which security headers are present: HSTS, CSP, clickjacking, mime sniffing, referrer, permissions, COOP, CORP. The headers are weighted and scored into a letter grade (A-F) in the style of securityheaders.com. Missing headers show the exact header line to add.

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
auger compare before.json after.json
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

## why

Most load testers are huge — a config file, a GUI or a whole platform just to measure a couple of endpoints. auger is one binary, a few flags and a fast release build.

## license

MIT
