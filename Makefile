.PHONY: help build test config clean up down

help:
	@echo "Available targets:"
	@echo "  build   - Build the local Docker image (rinha-rust-api:local)"
	@echo "  test    - Run Rust unit tests"
	@echo "  config  - Validate the local Docker Compose syntax"
	@echo "  up      - Start the local Docker Compose stack"
	@echo "  down    - Stop the local Docker Compose stack"
	@echo "  clean   - Clean cargo build artifacts"

build:
	@docker build -f docker/Dockerfile -t rinha-rust-api:local .

test:
	@cargo test

config:
	@docker compose -f docker-compose.local.yml config -q

up:
	@docker compose -f docker-compose.local.yml up -d

down:
	@docker compose -f docker-compose.local.yml down -v --remove-orphans

clean:
	@cargo clean
