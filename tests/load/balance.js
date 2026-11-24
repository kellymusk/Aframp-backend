/**
 * k6 load test — GET /balance
 *
 * Ramps to 100 concurrent virtual users over 30 s, holds for 60 s,
 * then ramps back down. Establishes a p95 latency baseline for the
 * balance endpoint, which is the most frequently polled endpoint in
 * production (merchants check it after every deposit notification).
 *
 * Usage:
 *   k6 run tests/load/balance.js
 *   BASE_URL=https://staging.example.com k6 run tests/load/balance.js
 *   AUTH_TOKEN=<jwt> k6 run tests/load/balance.js
 */

import http from "k6/http";
import { check, sleep } from "k6";
import { Trend, Rate } from "k6/metrics";

const BASE_URL = __ENV.BASE_URL || "http://localhost:3000";

// A pre-issued JWT for a seeded test merchant. In CI, generate this with
// `scripts/seed_load_test_user.sh` and pass it via AUTH_TOKEN.
// In local dev, copy the token from a successful /login response.
const AUTH_TOKEN = __ENV.AUTH_TOKEN || "";

// Custom metrics surfaced in the summary alongside the built-in ones.
const balanceDuration = new Trend("balance_req_duration", true);
const balanceErrors = new Rate("balance_error_rate");

export const options = {
  stages: [
    { duration: "30s", target: 100 }, // ramp up to 100 VUs
    { duration: "60s", target: 100 }, // hold
    { duration: "15s", target: 0 }, // ramp down
  ],
  thresholds: {
    // p95 must stay under 300 ms (see README baseline targets).
    "http_req_duration{endpoint:balance}": ["p(95)<300"],
    // Error rate must be below 1 %.
    balance_error_rate: ["rate<0.01"],
    // No requests should fail the HTTP-level check.
    checks: ["rate>0.99"],
  },
};

export default function () {
  const headers = {
    "Content-Type": "application/json",
    ...(AUTH_TOKEN ? { Authorization: `Bearer ${AUTH_TOKEN}` } : {}),
  };

  const res = http.get(`${BASE_URL}/balance`, {
    headers,
    tags: { endpoint: "balance" },
  });

  balanceDuration.add(res.timings.duration);
  balanceErrors.add(res.status >= 500);

  check(res, {
    "status is 200 or 401": (r) => r.status === 200 || r.status === 401,
    "response time < 500ms": (r) => r.timings.duration < 500,
  });

  sleep(0.5);
}
