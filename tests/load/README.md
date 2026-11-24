# Load Tests

Performance benchmarks using [k6](https://k6.io). Scripts live here; run them against a local or staging deployment — never production.

## Prerequisites

Install k6:
```bash
# macOS
brew install k6

# Ubuntu / Debian
sudo gpg -k
sudo gpg --no-default-keyring --keyring /usr/share/keyrings/k6-archive-keyring.gpg \
  --keyserver hkp://keyserver.ubuntu.com:80 --recv-keys C5AD17C747E3415A3642D57D77C6C491D6AC1D69
echo "deb [signed-by=/usr/share/keyrings/k6-archive-keyring.gpg] https://dl.k6.io/deb stable main" \
  | sudo tee /etc/apt/sources.list.d/k6.list
sudo apt-get update && sudo apt-get install k6
```

## Running the benchmarks

Start the backend locally (set `APP_ENV=development`, supply a real `DATABASE_URL`), then:

```bash
# GET /balance — ramp to 100 VUs over 30s, hold 60s
k6 run tests/load/balance.js

# GET /transactions — same ramp profile
k6 run tests/load/transactions.js

# Combined scenario (balance + transactions together)
k6 run tests/load/combined.js
```

Override the base URL:
```bash
BASE_URL=https://staging.example.com k6 run tests/load/balance.js
```

Override concurrency and duration:
```bash
k6 run --vus 50 --duration 30s tests/load/balance.js
```

## Baseline p95 latency targets

Established against a single-instance local deployment (Apple M2, Postgres on the same machine, no network hop):

| Endpoint | p95 target | Notes |
|---|---|---|
| `GET /health` | ≤ 50 ms | No auth, no DB query — purely a liveness check |
| `GET /balance` | ≤ 300 ms | One DB read behind JWT auth |
| `GET /transactions` | ≤ 500 ms | Paginated DB query behind JWT auth |

These are **regression thresholds**, not SLAs. If a code change pushes p95 past a target, investigate before merging.

Re-baseline after:
- Postgres schema changes (new indexes, schema migrations)
- Pool sizing changes (`DATABASE_MAX_CONNECTIONS`)
- Deployment topology changes (remote Postgres, more replicas)

## Interpreting results

k6 prints a summary at the end of each run. Key metrics:

- `http_req_duration` — full request latency including connection setup. The `p(95)` line is the p95.
- `http_req_failed` — fraction of requests that errored. Should be 0.00% under normal load; non-zero means the server is shedding requests.
- `vus` — concurrent virtual users active at peak.

A run with `checks` defined will also show pass/fail per threshold.
