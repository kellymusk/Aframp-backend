#!/bin/bash

# Test Database Setup Script
set -e

echo "🧪 Setting up test database environment"

if ! command -v sqlx >/dev/null 2>&1; then
    echo "❌ sqlx CLI not found. Install with:"
    echo "   cargo install sqlx-cli --no-default-features --features postgres"
    exit 1
fi

# Drop and recreate test database
echo "📊 Creating test database..."
dropdb aframp_test 2>/dev/null || true
createdb aframp_test

echo "📋 Running migrations with sqlx..."
DATABASE_URL=postgresql:///aframp_test sqlx migrate run

echo "✅ Migrations applied!"

echo ""
echo "Test database: aframp_test"
echo "Connection string: postgresql:///aframp_test"
echo ""
echo "To use in tests, set: DATABASE_URL=postgresql:///aframp_test"
echo ""
echo "To connect: psql -d aframp_test"
