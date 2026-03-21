# Makefile for clido

BINARY   := clido
DIST_DIR := dist
CARGO    := cargo

# Detect OS for cross-compile hints
UNAME_S := $(shell uname -s)

.PHONY: all build release dist clean test bench

all: build

## Build debug binary
build:
	$(CARGO) build

## Build optimized release binary and copy to dist/
release:
	$(CARGO) build --release
	mkdir -p $(DIST_DIR)
	cp target/release/$(BINARY) $(DIST_DIR)/$(BINARY)
	@echo "Release binary: $(DIST_DIR)/$(BINARY)"

## Alias for release
dist: release

## Install to ~/.cargo/bin
install:
	$(CARGO) install --path crates/clido-cli

## Run all workspace tests
test:
	$(CARGO) test --workspace

## Run startup benchmarks
bench:
	$(CARGO) bench -p clido-cli

## Remove dist/ directory
clean:
	rm -rf $(DIST_DIR)

## Run clippy
lint:
	$(CARGO) clippy --workspace -- -D warnings

## Format all source files
fmt:
	$(CARGO) fmt --all
