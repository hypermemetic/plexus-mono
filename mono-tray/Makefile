# Mono Tray — macOS menu bar player control
#
# Prerequisites:
#   - plexus-mono server running on port 4448 (for codegen)
#   - synapse-cc, bun, cargo-tauri installed

.PHONY: all dev build codegen codegen-force frontend app run restart install install-user clean backend backend-restart full full-app screenshots

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
run: install-user
	-pkill -f 'Mono Tray' 2>/dev/null; \
	for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do \
		pgrep -f 'Mono Tray' >/dev/null 2>&1 || break; \
		sleep 0.1; \
	done
	open ~/Applications/Mono\ Tray.app

# Kill and relaunch installed app (no rebuild)
restart:
	-pkill -f 'Mono Tray' 2>/dev/null; \
	for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do \
		pgrep -f 'Mono Tray' >/dev/null 2>&1 || break; \
		sleep 0.1; \
	done
	open ~/Applications/Mono\ Tray.app

# Install to /Applications (requires sudo)
install: app
	cp -r src-tauri/target/release/bundle/macos/Mono\ Tray.app /Applications/

# Install to ~/Applications (no sudo)
install-user: app
	mkdir -p ~/Applications
	cp -r src-tauri/target/release/bundle/macos/Mono\ Tray.app ~/Applications/

# Start the plexus-mono backend (port 4448)
backend:
	cd .. && cargo run --bin plexus-mono

# Kill existing backend and restart (polls for exit, max 2s)
backend-restart:
	-lsof -ti :4448 | xargs kill 2>/dev/null; \
	for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do \
		lsof -ti :4448 >/dev/null 2>&1 || break; \
		sleep 0.1; \
	done
	cd .. && cargo run --bin plexus-mono

# Full cycle: restart backend, regenerate clients, build & run app
# Codegen always forces by default; use SKIP_FORCE=1 to use cache
CODEGEN_FLAGS = --force
ifdef SKIP_FORCE
CODEGEN_FLAGS =
endif

BACKEND_BIN = $(abspath ../target/debug/plexus-mono)

# Detect if server needs restart: binary changed OR binary is newer than process.
full:
	@cd .. && cargo build --bin plexus-mono; \
	PID=$$(lsof -ti :4448 2>/dev/null | head -1); \
	if [ -z "$$PID" ]; then \
		echo "Server not running — starting..."; \
		"$(BACKEND_BIN)" & \
		synapse-cc wait music --url ws://127.0.0.1:4448 --timeout 15; \
	else \
		BIN_MOD=$$(stat -f %m "$(BACKEND_BIN)" 2>/dev/null || echo 0); \
		PROC_START=$$(ps -p $$PID -o lstart= 2>/dev/null); \
		PROC_EPOCH=$$(date -j -f "%a %b %d %T %Y" "$$PROC_START" +%s 2>/dev/null || echo 0); \
		if [ "$$BIN_MOD" -gt "$$PROC_EPOCH" ]; then \
			echo "Binary newer than running server — restarting..."; \
			kill $$PID 2>/dev/null; \
			for _i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do \
				kill -0 $$PID 2>/dev/null || break; \
				sleep 0.1; \
			done; \
			"$(BACKEND_BIN)" & \
			synapse-cc wait music --url ws://127.0.0.1:4448 --timeout 15; \
		else \
			echo "Server already running with latest binary."; \
		fi; \
	fi
	synapse-cc build $(CODEGEN_FLAGS)
	$(MAKE) run
	$(MAKE) screenshots

# Full cycle without backend restart: codegen, build & run app, screenshots
# Use when the backend is already running and you don't want to interrupt playback
full-app:
	synapse-cc build $(CODEGEN_FLAGS)
	$(MAKE) run
	$(MAKE) screenshots

# Generate documentation screenshots via Playwright (requires backend running)
screenshots:
	npx playwright test

# Clean build artifacts
clean:
	rm -rf dist
	rm -rf src-tauri/target
