# Stage 1: builder
FROM rust:1.85-bookworm AS builder

WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y \
    libasound2-dev \
    libdbus-1-dev \
    libsqlite3-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Copy source code
COPY . .

# Build library
RUN cargo build --lib --release

# Stage 2: test
FROM builder AS test

WORKDIR /app

# Run tests
RUN cargo test --lib --no-fail-fast

# Run clippy lints
RUN cargo clippy --lib -- -D warnings
