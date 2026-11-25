/**
 * k6 load test — GET /transactions
 *
 * Same ramp profile as balance.js (100 VUs / 60 s hold). The transactions
 * endpoint does a paginated DB query, so its p95 target is slightly higher.
 *
 * Usage:
 *   k6 run tests/load/transactions.js
 *   BASE_URL=https://staging.example.com k6 run tests/load/transactions.js
 */

import http from "k6/http";
import { check, sleep } from "k6";
import { Trend, Rate } from "k6/metrics";

const BASE_URL = __ENV.BASE_URL || "http://localhost:3000";
const AUTH_TOKEN = __ENV.AUTH_TOKEN || "";

const txDuration = new Trend("tx_req_duration", true);
const txErrors = new Rate("tx_error_rate");

export const options = {
  stages: [
    { duration: "30s", target: 100 },
    { duration: "60s", target: 100 },
    { duration: "15s", target: 0 },
  ],
  thresholds: {
    // p95 must stay under 500 ms (see README baseline targets).
    "http_req_duration{endpoint:transactions}": ["p(95)<500"],
    tx_error_rate: ["rate<0.01"],
    checks: ["rate>0.99"],
  },
};

export default function () {
  const headers = {
    "Content-Type": "application/json",
    ...(AUTH_TOKEN ? { Authorization: `Bearer ${AUTH_TOKEN}` } : {}),
  };

  // page=1&limit=20 matches the typical merchant dashboard poll.
  const res = http.get(`${BASE_URL}/transactions?page=1&limit=20`, {
    headers,
    tags: { endpoint: "transactions" },
  });

  txDuration.add(res.timings.duration);
  txErrors.add(res.status >= 500);

  check(res, {
    "status is 200 or 401": (r) => r.status === 200 || r.status === 401,
    "response time < 1000ms": (r) => r.timings.duration < 1000,
  });

  sleep(0.5);
}
