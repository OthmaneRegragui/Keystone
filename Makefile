.PHONY: dev build test clean migrate lint format run setup

# Development server
dev:
	APP_ENV=development cargo run --bin keystone

# Build release
build:
	cargo build --release

# Run all tests
# Each test binary runs against its own database (created on demand).
# RUST_TEST_THREADS=1 serializes tests within a binary so tests that truncate
# shared tables (see tests/helpers.rs::reset_db) are deterministic.
test:
	RUST_TEST_THREADS=1 APP_ENV=test cargo test

# Run tests with output
test-verbose:
	RUST_TEST_THREADS=1 APP_ENV=test cargo test -- --nocapture

# Clean build artifacts
clean:
	cargo clean

# Run database migrations
# Requires DATABASE_URL (or KEYSTONE__DATABASE__URL exported); run.sh exports it.
migrate:
	DATABASE_URL=postgres://keystone:keystone@localhost:5432/keystone sqlx migrate run

# Lint with clippy
lint:
	cargo clippy --workspace -- -D warnings

# Format code
format:
	cargo fmt --all

# Check formatting
format-check:
	cargo fmt --all -- --check

# Setup development environment
setup:
	cp -n .env.example .env || true
	mkdir -p data/storage

# Run in production mode
run:
	APP_ENV=production cargo run --bin keystone

# Build Tailwind CSS
css-dev:
	cd web && npx tailwindcss -i static/css/input.css -o static/css/app.css --watch

css-build:
	cd web && npx tailwindcss -i static/css/input.css -o static/css/app.css --minify

# Docker
docker-up:
	docker compose up -d

docker-down:
	docker compose down

docker-db-reset:
	./docker-run.sh db-reset

docker-logs:
	docker compose logs -f

# Check everything
check:
	cargo check --workspace
	cargo clippy --workspace -- -D warnings
	cargo fmt --all -- --check
