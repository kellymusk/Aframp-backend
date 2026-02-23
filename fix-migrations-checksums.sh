#!/bin/bash
# Fix SQLx migration checksums to match current migration file contents.
# Resolves: "migration <version> was previously applied but has been modified"

set -euo pipefail

DB_NAME="${1:-aframp}"

if [ -n "${DATABASE_URL:-}" ]; then
    PSQL_CMD=(psql "$DATABASE_URL")
    DB_LABEL="$DATABASE_URL"
else
    PSQL_CMD=(psql -d "$DB_NAME")
    DB_LABEL="$DB_NAME"
fi

echo "🔧 Fixing migration checksums for: $DB_LABEL"

if ! command -v sha256sum >/dev/null 2>&1; then
    echo "❌ sha256sum not found"
    exit 1
fi

if ! command -v psql >/dev/null 2>&1; then
    echo "❌ psql not found"
    exit 1
fi

tmp_sql="$(mktemp)"
trap 'rm -f "$tmp_sql"' EXIT

echo "-- Auto-generated checksum updates" > "$tmp_sql"
echo "BEGIN;" >> "$tmp_sql"

updated_count=0
skipped_count=0

while IFS= read -r file; do
    base="$(basename "$file")"
    version="${base%%_*}"

    if ! [[ "$version" =~ ^[0-9]+$ ]]; then
        echo "⚠️  Skipping invalid migration filename: $base"
        skipped_count=$((skipped_count + 1))
        continue
    fi

    hex_checksum="$(sha256sum "$file" | awk '{print $1}')"

    cat >> "$tmp_sql" <<EOF
UPDATE _sqlx_migrations
SET checksum = decode('$hex_checksum', 'hex')
WHERE version = $version;
EOF

    echo "  queued: $version -> $hex_checksum"
    updated_count=$((updated_count + 1))
done < <(find migrations -maxdepth 1 -type f -name '*.sql' | sort)

echo "COMMIT;" >> "$tmp_sql"

if [ "$updated_count" -eq 0 ]; then
    echo "❌ No migration SQL files found under ./migrations"
    exit 1
fi

"${PSQL_CMD[@]}" -v ON_ERROR_STOP=1 -f "$tmp_sql"

echo ""
echo "✅ Updated checksums for $updated_count migration file(s); skipped $skipped_count file(s)."
echo "📊 Current migration checksums:"
"${PSQL_CMD[@]}" -c "SELECT version, description, success, encode(checksum, 'hex') AS checksum FROM _sqlx_migrations ORDER BY version;"
echo ""
echo "✅ Retry: sqlx migrate run"
