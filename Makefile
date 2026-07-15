.PHONY: help setup build build-release run test fmt fmt-check clippy check \
        mongo-up mongo-down mongo-logs mongo-shell \
        docker-build docker-run \
        prod-up prod-down prod-logs prod-restart \
        sync-notion test-kraken replace-purchase sleeve-smoke \
        android-build clean

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'

setup: ## Create .env from the example template (does not overwrite an existing .env)
	@test -f .env || cp .env.example .env
	@echo ".env ready — fill in your API keys before running the bot."

## --- Local development (cargo) ---

build: ## Compile the bot (debug profile)
	cargo build

build-release: ## Compile the bot (release profile)
	cargo build --release

run: mongo-ensure ## Run the bot locally (debug profile, loads .env; auto-starts mongo if it's down)
	cargo run --bin eth-dca-bot

test: ## Run the unit test suite
	cargo test

fmt: ## Format the codebase
	cargo fmt --all

fmt-check: ## Check formatting without writing changes (CI-safe)
	cargo fmt --all -- --check

clippy: ## Lint with clippy (all targets)
	cargo clippy --all-targets

check: fmt-check clippy test ## Run the full verification suite (fmt + clippy + tests)

## --- MongoDB (local dev, via docker-compose.yml) ---

mongo-ensure: ## Start local MongoDB if it isn't already reachable on :27017
	@nc -z localhost 27017 2>/dev/null && echo "MongoDB already up on :27017" || ( \
		echo "MongoDB not reachable on :27017 — starting it..."; \
		$(MAKE) mongo-up; \
		printf "Waiting for MongoDB to accept connections"; \
		for i in $$(seq 1 30); do \
			nc -z localhost 27017 2>/dev/null && echo " up!" && exit 0; \
			printf "."; sleep 1; \
		done; \
		echo " timed out"; exit 1 \
	)

mongo-up: ## Start local MongoDB (docker-compose.yml)
	docker compose up -d

mongo-down: ## Stop local MongoDB
	docker compose down

mongo-logs: ## Tail local MongoDB logs
	docker compose logs -f

mongo-shell: ## Open a mongosh shell into the local dev database
	docker compose exec mongodb mongosh -u dca_user -p dca_password --authenticationDatabase dca_bot dca_bot

## --- Docker image ---

docker-build: ## Build the bot's production Docker image
	docker build -t eth-dca-bot .

docker-run: docker-build ## Run the built image standalone (expects MONGODB_URL etc. via --env-file)
	docker run --rm --env-file .env eth-dca-bot

## --- Production stack (docker-compose.prod.yml) ---

prod-up: ## Start the full production stack (bot + mongo), reading .env.prod
	docker compose -f docker-compose.prod.yml --env-file .env.prod up -d --build

prod-down: ## Stop the production stack
	docker compose -f docker-compose.prod.yml --env-file .env.prod down

prod-logs: ## Tail production stack logs
	docker compose -f docker-compose.prod.yml --env-file .env.prod logs -f

prod-restart: ## Restart just the bot container (e.g. after an env change)
	docker compose -f docker-compose.prod.yml --env-file .env.prod restart dca-bot

## --- Maintenance / diagnostic binaries (src/bin) ---

sync-notion: ## Check/sync the latest DCA purchase against Notion (needs BINANCE_* + NOTION_* env)
	cargo run --bin sync_notion

test-kraken: ## Manual Kraken smoke test: balance + price (read-only). Pass ARGS="--buy 5" or "--limit 5" to trade.
	cargo run --bin test_kraken -- $(ARGS)

replace-purchase: ## Replace one recorded purchase with another by order id: make replace-purchase OLD=<id> NEW=<id>
	cargo run --bin replace_purchase $(OLD) $(NEW)

sleeve-smoke: ## Limit-sleeve smoke harness (see docs/limit-sleeve-smoke-test.md). Pass ARGS="reconcile --chest 1.0 ..."
	cargo run --bin sleeve_smoke -- $(ARGS)

## --- Android cross-compilation ---

android-build: ## Cross-compile the bot for aarch64-linux-android (requires `cross`)
	cross build --target aarch64-linux-android --release

## --- Housekeeping ---

clean: ## Remove build artifacts
	cargo clean
