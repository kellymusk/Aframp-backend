/**
 * k6 load test — combined balance + transactions scenario
 *
 * Runs both endpoints concurrently with the same ramp profile. Each VU
 * alternates between balance and transactions to simulate a merchant
 * dashboard that polls both. Use this to find connection-pool exhaustion
 * or contention that only appears under mixed load.
 *
 * Usage:
 *   k6 run tests/load/combined.js
 */

import http from "k6/http";
import { check, sleep } from "k6";
import { Rate } from "k6/metrics";

const BASE_URL = __ENV.BASE_URL || "http://localhost:3000";
const AUTH_TOKEN = __ENV.AUTH_TOKEN || "";

const errorRate = new Rate("combined_error_rate");

export const options = {
  stages: [
    { duration: "30s", target: 100 },
    { duration: "60s", target: 100 },
    { duration: "15s", target: 0 },
  ],
  thresholds: {
    "http_req_duration{endpoint:balance}": ["p(95)<300"],
    "http_req_duration{endpoint:transactions}": ["p(95)<500"],
    combined_error_rate: ["rate<0.01"],
    checks: ["rate>0.99"],
  },
};

export default function () {
  const headers = {
    "Content-Type": "application/json",
    ...(AUTH_TOKEN ? { Authorization: `Bearer ${AUTH_TOKEN}` } : {}),
  };

  // Balance check
  const balRes = http.get(`${BASE_URL}/balance`, {
    headers,
    tags: { endpoint: "balance" },
  });
  errorRate.add(balRes.status >= 500);
  check(balRes, {
    "balance 200 or 401": (r) => r.status === 200 || r.status === 401,
    "balance < 500ms": (r) => r.timings.duration < 500,
  });

  sleep(0.2);

  // Transactions check
  const txRes = http.get(`${BASE_URL}/transactions?page=1&limit=20`, {
    headers,
    tags: { endpoint: "transactions" },
  });
  errorRate.add(txRes.status >= 500);
  check(txRes, {
    "transactions 200 or 401": (r) => r.status === 200 || r.status === 401,
    "transactions < 1000ms": (r) => r.timings.duration < 1000,
  });

  sleep(0.3);
}
