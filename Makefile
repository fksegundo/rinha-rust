.PHONY: help build build-lb up down logs test-k6 generate-extended test-k6-extended bench-local bench-extended clean-results

COMPOSE ?= docker compose -f submission/docker-compose.yml -f compose.local.yml
APP_IMAGE ?= rinha-rust-api:local
LB_IMAGE ?= rinha-lb:local
K6_IMAGE ?= grafana/k6:latest
LB_DIR ?= ../rinha-dotnetrust-lb
OFFICIAL_TEST_DIR ?= ../rinha-de-backend-2026-main/test
RESULTS_DIR ?= test
TEST_DATA_FILE ?= $(OFFICIAL_TEST_DIR)/test-data.json
EXTENDED_FACTOR ?= 2
EXTENDED_MODE ?= neutral
EXTENDED_TEST_DATA_FILE ?= $(RESULTS_DIR)/extended-test-data.json
RINHA_K6_TARGET_RATE ?= 900
RINHA_K6_DURATION ?= 120s
RINHA_K6_MAX_VUS ?= 250
RINHA_NATIVE_LEAF_SIZE ?= 48
RINHA_NATIVE_SCALE ?= 10000
RINHA_MAX_LEAF_VISITS ?= 0
RINHA_SEARCH_MODE ?= key-first
API_CPU_LIMIT ?= 0.42
LB_CPU_LIMIT ?= 0.16
API_MEMORY_LIMIT ?= 165M
LB_MEMORY_LIMIT ?= 20M
LB_WORKERS ?= 2

help:
	@echo "Targets:"
	@echo "  build       Build local API image ($(APP_IMAGE))"
	@echo "  build-lb    Build local LB image ($(LB_IMAGE))"
	@echo "  up          Start local stack"
	@echo "  down        Stop local stack"
	@echo "  test-k6     Run official k6 workload and write $(RESULTS_DIR)/results.json"
	@echo "  generate-extended Generate larger/reordered test-data JSON"
	@echo "  test-k6-extended  Run k6 with generated extended dataset"
	@echo "  bench-local Build, run stack, execute k6, and stop stack"
	@echo "  bench-extended Build, run stack, execute extended k6, and stop stack"
	@echo "  logs        Follow compose logs"

build:
	@docker build \
		--build-arg RINHA_NATIVE_LEAF_SIZE=$(RINHA_NATIVE_LEAF_SIZE) \
		--build-arg RINHA_NATIVE_SCALE=$(RINHA_NATIVE_SCALE) \
		-f submission/Dockerfile \
		-t $(APP_IMAGE) .

build-lb:
	@docker build -f $(LB_DIR)/Dockerfile -t $(LB_IMAGE) $(LB_DIR)

up:
	@APP_IMAGE=$(APP_IMAGE) LB_IMAGE=$(LB_IMAGE) RINHA_MAX_LEAF_VISITS=$(RINHA_MAX_LEAF_VISITS) RINHA_SEARCH_MODE=$(RINHA_SEARCH_MODE) API_CPU_LIMIT=$(API_CPU_LIMIT) LB_CPU_LIMIT=$(LB_CPU_LIMIT) API_MEMORY_LIMIT=$(API_MEMORY_LIMIT) LB_MEMORY_LIMIT=$(LB_MEMORY_LIMIT) LB_WORKERS=$(LB_WORKERS) $(COMPOSE) up -d --force-recreate
	@echo "Waiting for /ready..."
	@for i in $$(seq 1 60); do \
		if curl -sf http://localhost:9999/ready >/dev/null; then echo "ready"; exit 0; fi; \
		sleep 1; \
	done; \
	echo "service did not become ready"; exit 1

down:
	@APP_IMAGE=$(APP_IMAGE) LB_IMAGE=$(LB_IMAGE) $(COMPOSE) down -v --remove-orphans

logs:
	@$(COMPOSE) logs -f

clean-results:
	@rm -f $(RESULTS_DIR)/results.json $(RESULTS_DIR)/k6_summary.json $(RESULTS_DIR)/test-data.json

test-k6: clean-results
	@test -f "$(TEST_DATA_FILE)" || (echo "Missing $(TEST_DATA_FILE)" && exit 1)
	@mkdir -p $(RESULTS_DIR)
	@cp "$(TEST_DATA_FILE)" "$(RESULTS_DIR)/test-data.json"
	@chmod 777 $(RESULTS_DIR)
	@docker run --rm -i \
		--network host \
		-e API_URL="http://localhost:9999/fraud-score" \
		-e TEST_DATA="/test/test-data.json" \
		-e RINHA_K6_TARGET_RATE="$(RINHA_K6_TARGET_RATE)" \
		-e RINHA_K6_DURATION="$(RINHA_K6_DURATION)" \
		-e RINHA_K6_MAX_VUS="$(RINHA_K6_MAX_VUS)" \
		-v "$(PWD)/test/test.js:/scripts/test.js:ro" \
		-v "$(PWD)/$(RESULTS_DIR):/test" \
		-w / \
		$(K6_IMAGE) \
		run --summary-trend-stats="p(50),p(95),p(99)" /scripts/test.js
	@cat $(RESULTS_DIR)/results.json

generate-extended:
	@python3 scripts/generate_extended_test_data.py \
		--input "$(OFFICIAL_TEST_DIR)/test-data.json" \
		--output "$(EXTENDED_TEST_DATA_FILE)" \
		--factor "$(EXTENDED_FACTOR)" \
		--mode "$(EXTENDED_MODE)"

test-k6-extended: generate-extended
	@$(MAKE) test-k6 \
		TEST_DATA_FILE="$(EXTENDED_TEST_DATA_FILE)" \
		RINHA_K6_DURATION=241s \
		RINHA_K6_TARGET_RATE="$(RINHA_K6_TARGET_RATE)" \
		RINHA_K6_MAX_VUS="$(RINHA_K6_MAX_VUS)"

bench-local: build build-lb up test-k6 down

bench-extended: build build-lb up test-k6-extended down

# Diagnostic bench: keeps containers up after k6, dumps API logs, then tears down.
bench-diag: build build-lb up test-k6 capture-logs down

capture-logs:
	@mkdir -p $(RESULTS_DIR)
	@$(COMPOSE) logs --no-color api1 api2 > $(RESULTS_DIR)/diag-api-logs.txt 2>&1 || true
	@docker stats --no-stream --format '{{.Name}} {{.CPUPerc}} {{.MemUsage}} {{.MemPerc}}' > $(RESULTS_DIR)/docker-stats.txt 2>&1 || true
	@echo "API logs captured to $(RESULTS_DIR)/diag-api-logs.txt"
	@echo "Docker stats captured to $(RESULTS_DIR)/docker-stats.txt"
	@echo "Last lines:"
	@tail -30 $(RESULTS_DIR)/diag-api-logs.txt
