# auger

![License](https://img.shields.io/badge/license-MIT-blue)
![CI](https://github.com/user123cy/auger/actions/workflows/ci.yml/badge.svg)

Load test, discover and inspect HTTP endpoints from the terminal.

Three commands, one binary:

- `auger run` — hammer a URL with concurrent requests, get percentiles, a latency histogram and a flamegraph
- `auger scan` — brute-force endpoints with a wordlist
- `auger check` — inspect status, HTTP version and security headers

## install

```
cargo install --path .
```

## load test

```
auger run https://api.example.com/                  # 20 workers, 5s
auger run https://api.example.com/ -c 200 -d 30s    # 200 workers, 30s
auger run https://api.example.com/ -m POST -d 10s
auger run https://api.example.com/ --random-ua -H "X-Token: abc"
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
```

## inspect

```
auger check https://example.com/
```

Shows status, HTTP version, server and which security headers are present: HSTS, CSP, clickjacking, mime sniffing, referrer, permissions, COOP, CORP.

## reports

```
auger run https://api.example.com/ -d 10s -s report.json
auger report report.json            # print it again later
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
