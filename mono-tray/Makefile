# Mono Tray — macOS menu bar player control
#
# Prerequisites:
#   - plexus-mono server running on port 4448 (for codegen)
#   - synapse-cc, bun, cargo-tauri installed

.PHONY: all dev build codegen codegen-force frontend app run restart install install-user clean backend backend-restart full

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
	-pkill -f 'Mono Tray' 2>/dev/null; sleep 0.5
	open ~/Applications/Mono\ Tray.app

# Kill and relaunch installed app (no rebuild)
restart:
	-pkill -f 'Mono Tray' 2>/dev/null; sleep 0.5
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

# Kill existing backend and restart
backend-restart:
	-lsof -ti :4448 | xargs kill 2>/dev/null; sleep 0.5
	cd .. && cargo run --bin plexus-mono

# Full cycle: restart backend, regenerate clients, build & run app
# Use: make full FORCE_CODEGEN=1 to force client regeneration
CODEGEN_FLAGS =
ifdef FORCE_CODEGEN
CODEGEN_FLAGS = --force
endif

BACKEND_BIN = $(abspath ../target/debug/plexus-mono)

# Detect if server needs restart: binary changed OR binary is newer than process.
full:
	@cd .. && cargo build --bin plexus-mono; \
	PID=$$(lsof -ti :4448 2>/dev/null | head -1); \
	if [ -z "$$PID" ]; then \
		echo "Server not running — starting..."; \
		"$(BACKEND_BIN)" & \
		synapse-cc wait monochrome --url ws://127.0.0.1:4448 --timeout 15; \
	else \
		BIN_MOD=$$(stat -f %m "$(BACKEND_BIN)" 2>/dev/null || echo 0); \
		PROC_START=$$(ps -p $$PID -o lstart= 2>/dev/null); \
		PROC_EPOCH=$$(date -j -f "%a %b %d %T %Y" "$$PROC_START" +%s 2>/dev/null || echo 0); \
		if [ "$$BIN_MOD" -gt "$$PROC_EPOCH" ]; then \
			echo "Binary newer than running server — restarting..."; \
			kill $$PID 2>/dev/null; sleep 0.5; \
			"$(BACKEND_BIN)" & \
			synapse-cc wait monochrome --url ws://127.0.0.1:4448 --timeout 15; \
		else \
			echo "Server already running with latest binary."; \
		fi; \
	fi
	synapse-cc build $(CODEGEN_FLAGS)
	$(MAKE) run

# Clean build artifacts
clean:
	rm -rf dist
	rm -rf src-tauri/target
