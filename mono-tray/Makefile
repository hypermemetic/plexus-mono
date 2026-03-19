# Mono Tray — macOS menu bar player control
#
# Prerequisites:
#   - plexus-mono server running on port 4448 (for codegen)
#   - synapse-cc, bun, cargo-tauri installed
#
# NOTE: `make full` lives in the mono-provider crate (../../mono-provider/Makefile)
# which builds the binary, starts the server, runs codegen, and builds the app.

.PHONY: all dev build codegen codegen-force frontend app run restart install install-user clean backend backend-restart full full-app screenshots

PROVIDER_DIR = $(abspath ../../mono-provider)
BACKEND_BIN = $(abspath ../../mono-provider/target/debug/plexus-mono)

# Full build: codegen → frontend → native app
all: codegen frontend app

# Dev mode: rebuild frontend + launch Tauri dev server
dev: frontend
	cargo tauri dev

# Generate TypeScript clients from live servers
# Single config handles both monochrome and substrate targets
codegen:
	synapse-cc build

# Force regeneration (ignore cache)
codegen-force:
	synapse-cc build --force

# Bundle TypeScript frontend
frontend:
	bun run build

# Build native macOS app (release)
app: frontend
	cargo tauri build

# Build, install to ~/Applications, kill old instance, and launch
# Retry open for up to 10s to handle transient procNotFound after quit
run: install-user
	-osascript -e 'tell application "Mono Tray" to quit' 2>/dev/null
	@for i in $$(seq 1 20); do \
		open ~/Applications/Mono\ Tray.app 2>/dev/null && exit 0; \
		sleep 0.5; \
	done; echo "Failed to open Mono Tray after 10s" && exit 1

# Kill and relaunch installed app (no rebuild)
restart:
	-osascript -e 'tell application "Mono Tray" to quit' 2>/dev/null
	@for i in $$(seq 1 20); do \
		open ~/Applications/Mono\ Tray.app 2>/dev/null && exit 0; \
		sleep 0.5; \
	done; echo "Failed to open Mono Tray after 10s" && exit 1

# Install to /Applications (requires sudo)
install: app
	cp -r src-tauri/target/release/bundle/macos/Mono\ Tray.app /Applications/

# Install to ~/Applications (no sudo)
install-user: app
	mkdir -p ~/Applications
	cp -r src-tauri/target/release/bundle/macos/Mono\ Tray.app ~/Applications/

# Start the plexus-mono backend (port 4448)
backend:
	cd $(PROVIDER_DIR) && cargo run --bin plexus-mono

# Kill existing backend and restart
backend-restart:
	-lsof -ti :4448 -sTCP:LISTEN | xargs kill 2>/dev/null; \
	while lsof -ti :4448 -sTCP:LISTEN >/dev/null 2>&1; do :; done
	cd $(PROVIDER_DIR) && cargo run --bin plexus-mono

# Codegen flags: force by default; use SKIP_FORCE=1 to use cache
CODEGEN_FLAGS = --force
ifdef SKIP_FORCE
CODEGEN_FLAGS =
endif

# Full cycle: build binary, (re)start backend, codegen, build & run app, screenshots
# Delegates to mono-provider's Makefile
full:
	$(MAKE) -C $(PROVIDER_DIR) full

# Full cycle without backend restart: codegen, build & run app, screenshots
full-app:
	$(MAKE) -C $(PROVIDER_DIR) full-app

# Generate documentation screenshots via Playwright (requires backend running)
screenshots:
	npx playwright test

# Clean build artifacts
clean:
	rm -rf dist
	rm -rf src-tauri/target
