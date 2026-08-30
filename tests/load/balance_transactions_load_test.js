import http from 'k6/http';
import { check, group, sleep } from 'k6';

// Configuration
const BASE_URL = __ENV.BASE_URL || 'http://localhost:8000';
const RAMP_UP_DURATION = '1m';
const SUSTAIN_DURATION = '5m';
const RAMP_DOWN_DURATION = '1m';

// VU stages: ramp up to 100 concurrent users
export const options = {
  stages: [
    { duration: RAMP_UP_DURATION, target: 100 },  // ramp up to 100 VUs
    { duration: SUSTAIN_DURATION, target: 100 },  // sustain at 100 VUs for 5 min
    { duration: RAMP_DOWN_DURATION, target: 0 },  // ramp down to 0 VUs
  ],
  thresholds: {
    http_req_duration: ['p(95)<500', 'p(99)<1000'], // p95 < 500ms, p99 < 1s
    http_req_failed: ['rate<0.1'],                  // error rate must be < 10%
  },
};

// Mock wallet data for testing
const mockWallets = [
  'GBVVRXLMRYAASFU2HI6FOWZFJC5B5VPVW4XT7NZQA56BTJYDP6KSHJA',
  'GBUQWP3BOUZX34ULNQG23RQ6F4BVXEYMJUCHUOLZMSKSVKNQG77YLVXQ',
  'GCVLWV37D7ASJGFVWQW4KZU4VNKVQYZKP3WN5YXLV2CWWPV4RVSWSCT',
  'GDJ4VTK5C76XSXYHDHQO27HQLTCQFASBCGSYUZAVSQ2ZLA4BFNGQHVEK',
  'GASOCNZ3C4W34B277LGVP7QDGXDNFG44K7RCVMHUVVDSYAHKRX4GDJJ',
];

// Mock auth token (would be actual JWT in real test)
const authToken = __ENV.AUTH_TOKEN || 'test-token-12345';

export default function () {
  // Set base URL and common headers
  const params = {
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${authToken}`,
    },
    timeout: '10s',
  };

  // Randomly select a wallet
  const walletId = mockWallets[Math.floor(Math.random() * mockWallets.length)];

  group('Balance Endpoint', () => {
    const balanceRes = http.get(`${BASE_URL}/balance?wallet=${walletId}`, params);

    check(balanceRes, {
      'status is 200': (r) => r.status === 200,
      'response time < 500ms': (r) => r.timings.duration < 500,
      'has balance field': (r) => r.json('balance') !== undefined,
    });

    // Log p95 latency for this request
    console.log(`/balance p95: ${balanceRes.timings.duration}ms`);
  });

  sleep(0.5);

  group('Transactions Endpoint', () => {
    const txRes = http.get(`${BASE_URL}/transactions?wallet=${walletId}&limit=20`, params);

    check(txRes, {
      'status is 200': (r) => r.status === 200,
      'response time < 500ms': (r) => r.timings.duration < 500,
      'has transactions array': (r) => Array.isArray(r.json('transactions')),
    });

    // Log p95 latency for this request
    console.log(`/transactions p95: ${txRes.timings.duration}ms`);
  });

  sleep(1);
}

export function handleSummary(data) {
  // Log summary to console
  console.log('Load Test Summary:');
  console.log(`Total requests: ${data.metrics.http_reqs.value}`);
  console.log(`Failed requests: ${data.metrics.http_req_failed.value}`);
  console.log(`P95 latency: ${Math.round(data.metrics.http_req_duration.values.p(95))}ms`);
  console.log(`P99 latency: ${Math.round(data.metrics.http_req_duration.values.p(99))}ms`);

  return {
    stdout: textSummary(data, { indent: ' ', enableColors: true }),
    'summary.json': JSON.stringify(data),
  };
}

function textSummary(data, options = {}) {
  const { indent = '', enableColors = false } = options;
  const color = (text, code) => enableColors ? `\x1b[${code}m${text}\x1b[0m` : text;

  let summary = '\n';
  summary += `${indent}█ /balance Endpoint\n`;
  summary += `${indent}  ├─ Avg latency: ${Math.round(data.metrics.http_req_duration.values.avg)}ms\n`;
  summary += `${indent}  ├─ P95 latency: ${Math.round(data.metrics.http_req_duration.values['p(95)'])}ms\n`;
  summary += `${indent}  ├─ P99 latency: ${Math.round(data.metrics.http_req_duration.values['p(99)'])}ms\n`;
  summary += `${indent}  └─ Success rate: ${((1 - data.metrics.http_req_failed.value / data.metrics.http_reqs.value) * 100).toFixed(2)}%\n`;

  return summary;
}
