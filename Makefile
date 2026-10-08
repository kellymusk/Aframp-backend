.PHONY: run test migrate fmt clippy docker docker-db

run:
	OTP_PROVIDER=mock cargo run

test:
	TEST_DATABASE_URL="$${TEST_DATABASE_URL:-postgres://postgres:postgres@localhost:5432/aframp_test}" cargo test

migrate:
	DATABASE_URL="$${DATABASE_URL:-postgres://postgres:postgres@localhost:5432/aframp}" sqlx migrate run

fmt:
	cargo fmt --all

clippy:
	cargo clippy -- -D warnings

docker:
	docker build -t aframp-backend .

docker-db:
	docker start aframp-postgres >/dev/null 2>&1 || docker run -d --name aframp-postgres \
		-e POSTGRES_USER=postgres \
		-e POSTGRES_PASSWORD=postgres \
		-e POSTGRES_DB=aframp \
		-p 5432:5432 \
		postgres:16
