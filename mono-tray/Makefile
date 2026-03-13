# Mono Tray — macOS menu bar player control
#
# Prerequisites:
#   - plexus-mono server running on port 4448 (for codegen)
#   - synapse-cc, bun, cargo-tauri installed

.PHONY: all dev build codegen codegen-force frontend app install clean

# Full build: codegen → frontend → native app
all: codegen frontend app

# Dev mode: rebuild frontend + launch Tauri dev server
dev: frontend
	cargo tauri dev

# Generate TypeScript client from live plexus-mono server
# Uses synapse.config.json for backend, port, transport, output dir
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

# Install to /Applications
install: app
	cp -r src-tauri/target/release/bundle/macos/Mono\ Tray.app /Applications/

# Clean build artifacts
clean:
	rm -rf dist
	rm -rf src-tauri/target
