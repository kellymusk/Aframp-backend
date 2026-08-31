import http from 'k6/http';
import { check, group, sleep } from 'k6';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8000';
const RAMP_UP_DURATION = '1m';
const SUSTAIN_DURATION = '5m';
const RAMP_DOWN_DURATION = '1m';

export const options = {
  stages: [
    { duration: RAMP_UP_DURATION, target: 100 },
    { duration: SUSTAIN_DURATION, target: 100 },
    { duration: RAMP_DOWN_DURATION, target: 0 },
  ],
  thresholds: {
    http_req_duration: ['p(95)<500', 'p(99)<1000'],
    http_req_failed: ['rate<0.1'],
  },
};

const mockWallets = [
  'GBVVRXLMRYAASFU2HI6FOWZFJC5B5VPVW4XT7NZQA56BTJYDP6KSHJA',
  'GBUQWP3BOUZX34ULNQG23RQ6F4BVXEYMJUCHUOLZMSKSVKNQG77YLVXQ',
  'GCVLWV37D7ASJGFVWQW4KZU4VNKVQYZKP3WN5YXLV2CWWPV4RVSWSCT',
  'GDJ4VTK5C76XSXYHDHQO27HQLTCQFASBCGSYUZAVSQ2ZLA4BFNGQHVEK',
  'GASOCNZ3C4W34B277LGVP7QDGXDNFG44K7RCVMHUVVDSYAHKRX4GDJJ',
];

const authToken = __ENV.AUTH_TOKEN || 'test-token-12345';

export default function () {
  const params = {
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${authToken}`,
    },
    timeout: '10s',
  };

  const walletId = mockWallets[Math.floor(Math.random() * mockWallets.length)];

  group('Balance Endpoint', () => {
    const balanceRes = http.get(`${BASE_URL}/balance?wallet=${walletId}`, params);
    check(balanceRes, {
      'status is 200': (r) => r.status === 200,
      'response time < 500ms': (r) => r.timings.duration < 500,
      'has balance field': (r) => r.json('balance') !== undefined,
    });
  });

  sleep(0.5);

  group('Transactions Endpoint', () => {
    const txRes = http.get(`${BASE_URL}/transactions?wallet=${walletId}&limit=20`, params);
    check(txRes, {
      'status is 200': (r) => r.status === 200,
      'response time < 500ms': (r) => r.timings.duration < 500,
      'has transactions array': (r) => Array.isArray(r.json('transactions')),
    });
  });

  sleep(1);
}
