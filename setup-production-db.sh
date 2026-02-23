#!/bin/bash

# Production Database Setup Script
# This script sets up the production database with proper security and optimizations

set -e

echo "🏭 Setting up PRODUCTION database environment"
echo "⚠️  WARNING: This will set up the production database!"
echo ""

# Confirm production setup
read -p "Are you sure you want to set up the PRODUCTION database? (yes/no): " confirm
if [ "$confirm" != "yes" ]; then
    echo "❌ Setup cancelled"
    exit 1
fi

DB_NAME="aframp"
DB_USER="${DB_USER:-aframp_user}"
DB_PASSWORD="${DB_PASSWORD:-$(openssl rand -base64 32)}"

echo ""
echo "📊 Database Configuration:"
echo "  Database: $DB_NAME"
echo "  User: $DB_USER"
echo "  Password: [generated securely]"
echo ""
echo "  Password value: $DB_PASSWORD"
echo ""

if ! command -v sqlx >/dev/null 2>&1; then
    echo "🔧 Installing sqlx CLI..."
    cargo install sqlx-cli --no-default-features --features postgres
fi

# Create database and user
echo "📊 Creating production database and user..."
sudo -u postgres psql << EOF
-- Create database if it does not exist
SELECT 'CREATE DATABASE $DB_NAME'
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = '$DB_NAME')\gexec

-- Create user if it does not exist
DO \$\$
BEGIN
   IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = '$DB_USER') THEN
      CREATE ROLE $DB_USER LOGIN PASSWORD '$DB_PASSWORD';
   ELSE
      ALTER ROLE $DB_USER WITH LOGIN PASSWORD '$DB_PASSWORD';
   END IF;
END
\$\$;

-- Grant privileges
GRANT ALL PRIVILEGES ON DATABASE $DB_NAME TO $DB_USER;

-- Connect to the database and grant schema privileges
\c $DB_NAME

-- Grant schema privileges
GRANT ALL ON SCHEMA public TO $DB_USER;
GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA public TO $DB_USER;
GRANT ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA public TO $DB_USER;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON TABLES TO $DB_USER;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON SEQUENCES TO $DB_USER;

-- Enable required extensions
CREATE EXTENSION IF NOT EXISTS "pgcrypto";
CREATE EXTENSION IF NOT EXISTS "pg_stat_statements";

-- Configure for production
ALTER DATABASE $DB_NAME SET log_statement = 'mod';
ALTER DATABASE $DB_NAME SET log_min_duration_statement = 1000;
EOF

echo "✅ Database and user created!"

# Run migrations
echo "📋 Running database migrations..."
# Avoid URL-parsing issues when password contains reserved characters (/, +, @, :)
PGPASSWORD="$DB_PASSWORD" DATABASE_URL="postgresql://$DB_USER@localhost/$DB_NAME" sqlx migrate run
echo "✅ Migrations applied!"

# Create backup directory
echo "💾 Setting up backup directory..."
sudo mkdir -p /var/backups/postgresql/$DB_NAME
sudo chown postgres:postgres /var/backups/postgresql/$DB_NAME
echo "✅ Backup directory created!"

# Create backup script
echo "📝 Creating backup script..."
cat > backup-db.sh << 'BACKUP_SCRIPT'
#!/bin/bash
# Database Backup Script

BACKUP_DIR="/var/backups/postgresql/aframp"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
BACKUP_FILE="$BACKUP_DIR/aframp_backup_$TIMESTAMP.sql.gz"

# Create backup
pg_dump -U aframp_user aframp | gzip > $BACKUP_FILE

# Keep only last 7 days of backups
find $BACKUP_DIR -name "aframp_backup_*.sql.gz" -mtime +7 -delete

echo "Backup completed: $BACKUP_FILE"
BACKUP_SCRIPT

chmod +x backup-db.sh
echo "✅ Backup script created: ./backup-db.sh"

# Create production .env file
echo "📝 Creating production .env file..."
cat > .env.production << ENV_FILE
# Production Environment Configuration
DATABASE_URL=postgresql://$DB_USER:$DB_PASSWORD@localhost/$DB_NAME
REDIS_URL=redis://127.0.0.1:6379
RUST_LOG=info
SERVER_HOST=0.0.0.0
SERVER_PORT=8000
DB_MAX_CONNECTIONS=50
DB_MIN_CONNECTIONS=10
DB_CONNECTION_TIMEOUT=30
DB_IDLE_TIMEOUT=300
CACHE_MAX_CONNECTIONS=50
CACHE_DEFAULT_TTL=3600
STELLAR_NETWORK=mainnet
STELLAR_REQUEST_TIMEOUT=30
STELLAR_MAX_RETRIES=3
STELLAR_HEALTH_CHECK_INTERVAL=60

# Security
# Add your secret keys here
# PAYSTACK_SECRET_KEY=your_secret_key_here
# JWT_SECRET=your_jwt_secret_here
ENV_FILE

echo "✅ Production .env file created: .env.production"

# Create systemd service file
echo "📝 Creating systemd service..."
cat > aframp-backend.service << SERVICE_FILE
[Unit]
Description=Aframp Backend Service
After=network.target postgresql.service redis.service

[Service]
Type=simple
User=$USER
WorkingDirectory=$(pwd)
EnvironmentFile=$(pwd)/.env.production
ExecStart=$(pwd)/target/release/Aframp-Backend
Restart=always
RestartSec=10

# Security hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=$(pwd)/logs

# Logging
StandardOutput=journal
StandardError=journal
SyslogIdentifier=aframp-backend

[Install]
WantedBy=multi-user.target
SERVICE_FILE

echo "✅ Systemd service file created: aframp-backend.service"
echo ""
echo "To install the service:"
echo "  sudo cp aframp-backend.service /etc/systemd/system/"
echo "  sudo systemctl daemon-reload"
echo "  sudo systemctl enable aframp-backend"
echo "  sudo systemctl start aframp-backend"

# Create monitoring script
echo "📝 Creating monitoring script..."
cat > monitor-db.sh << 'MONITOR_SCRIPT'
#!/bin/bash
# Database Monitoring Script

DB_NAME="aframp"

echo "=== Database Status ==="
psql -U aframp_user -d $DB_NAME -c "
SELECT 
    schemaname,
    tablename,
    pg_size_pretty(pg_total_relation_size(schemaname||'.'||tablename)) AS size,
    n_live_tup as rows
FROM pg_stat_user_tables
ORDER BY pg_total_relation_size(schemaname||'.'||tablename) DESC;
"

echo ""
echo "=== Active Connections ==="
psql -U aframp_user -d $DB_NAME -c "
SELECT count(*) as active_connections 
FROM pg_stat_activity 
WHERE state = 'active';
"

echo ""
echo "=== Slow Queries (>1s) ==="
psql -U aframp_user -d $DB_NAME -c "
SELECT 
    query,
    calls,
    total_time,
    mean_time,
    max_time
FROM pg_stat_statements
WHERE mean_time > 1000
ORDER BY mean_time DESC
LIMIT 10;
"

echo ""
echo "=== Cache Hit Ratio ==="
psql -U aframp_user -d $DB_NAME -c "
SELECT 
    sum(heap_blks_read) as heap_read,
    sum(heap_blks_hit) as heap_hit,
    sum(heap_blks_hit) / (sum(heap_blks_hit) + sum(heap_blks_read)) as ratio
FROM pg_statio_user_tables;
"
MONITOR_SCRIPT

chmod +x monitor-db.sh
echo "✅ Monitoring script created: ./monitor-db.sh"

echo ""
echo "🎉 Production database setup complete!"
echo ""
echo "📋 Summary:"
echo "  Database: $DB_NAME"
echo "  User: $DB_USER"
echo "  Password: $DB_PASSWORD"
echo ""
echo "⚠️  IMPORTANT: Save these credentials securely!"
echo "  They have been written to .env.production"
echo ""
echo "📝 Next steps:"
echo "  1. Review and update .env.production with your API keys"
echo "  2. Build release binary: cargo build --release"
echo "  3. Test the connection: psql -U $DB_USER -d $DB_NAME"
echo "  4. Run the backend: cargo run --release"
echo "  5. Set up systemd service (see instructions above)"
echo "  6. Configure automated backups (cron job for backup-db.sh)"
echo ""
echo "📊 Useful commands:"
echo "  Monitor database: ./monitor-db.sh"
echo "  Backup database: ./backup-db.sh"
echo "  Connect to DB: psql -U $DB_USER -d $DB_NAME"
echo ""
