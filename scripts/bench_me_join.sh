#!/usr/bin/env bash
# Benchmark: two sequential /me queries vs one LEFT JOIN (#1128).
#
# Usage:
#   DATABASE_URL=postgres://postgres:postgres@localhost:5432/aframp \
#     ./scripts/bench_me_join.sh [iterations]
#
# Requires psql. Safe to re-run — uses a throwaway email prefix.

set -euo pipefail

DB="${DATABASE_URL:?DATABASE_URL is required}"
ITERS="${1:-200}"
PREFIX="bench_me_join_$(date +%s)"

echo "==> seeding ${ITERS} users+merchants (prefix=${PREFIX})"
psql "$DB" -v ON_ERROR_STOP=1 <<SQL
DO \$\$
DECLARE
  i int;
  uid uuid;
BEGIN
  FOR i IN 1..${ITERS} LOOP
    INSERT INTO users (email, password_hash, name, phone_number, phone_verified)
    VALUES ('${PREFIX}_' || i || '@example.com', 'x', 'Bench', '+234800000' || lpad(i::text, 4, '0'), true)
    RETURNING id INTO uid;
    INSERT INTO merchants (user_id, name) VALUES (uid, 'Bench');
  END LOOP;
END \$\$;
SQL

USER_ID=$(psql "$DB" -Atc "SELECT id FROM users WHERE email = '${PREFIX}_1@example.com'")

echo "==> two-query path × ${ITERS}"
TWO_MS=$(psql "$DB" -Atc "
\\timing on
SELECT 1 FROM users WHERE id = '${USER_ID}';
SELECT 1 FROM merchants WHERE user_id = '${USER_ID}' LIMIT 1;
" 2>&1 | awk '/Time:/ {print; exit}')

# Warm + measure with a plpgsql loop so we get one comparable number.
TWO_TOTAL=$(psql "$DB" -Atc "
SELECT (EXTRACT(EPOCH FROM clock_timestamp() - t0) * 1000)::int
FROM (
  SELECT clock_timestamp() AS t0, NULL
  FROM generate_series(1, ${ITERS}) g,
  LATERAL (SELECT id FROM users WHERE id = '${USER_ID}') u,
  LATERAL (SELECT id FROM merchants WHERE user_id = '${USER_ID}' LIMIT 1) m
) s;
")

JOIN_TOTAL=$(psql "$DB" -Atc "
SELECT (EXTRACT(EPOCH FROM clock_timestamp() - t0) * 1000)::int
FROM (
  SELECT clock_timestamp() AS t0, NULL
  FROM generate_series(1, ${ITERS}) g,
  LATERAL (
    SELECT u.id, m.id
      FROM users u
      LEFT JOIN merchants m ON m.user_id = u.id
     WHERE u.id = '${USER_ID}'
     LIMIT 1
  ) j
) s;
")

echo "two sequential queries × ${ITERS}: ${TWO_TOTAL} ms"
echo "single LEFT JOIN     × ${ITERS}: ${JOIN_TOTAL} ms"
if [[ "${TWO_TOTAL}" -gt 0 ]]; then
  python3 - <<PY
two=${TWO_TOTAL}
join=${JOIN_TOTAL}
print(f"speedup: {two/join:.2f}x" if join else "speedup: n/a")
PY
fi

echo "==> cleanup"
psql "$DB" -v ON_ERROR_STOP=1 <<SQL
DELETE FROM merchants WHERE user_id IN (SELECT id FROM users WHERE email LIKE '${PREFIX}_%');
DELETE FROM users WHERE email LIKE '${PREFIX}_%';
SQL

echo "done. (sample timing probe: ${TWO_MS:-n/a})"
