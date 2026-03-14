# =============================================================================
# rust-brain — Ingestion Service Dockerfile
# =============================================================================
# Macro expansion and source ingestion service
# =============================================================================

# Build stage
FROM rust:1.85-slim AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy manifests first for better caching
COPY Cargo.toml ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release && rm -rf src

# Copy actual source and build
COPY src ./src
RUN touch src/main.rs && cargo build --release

# Runtime stage
FROM rust:1.85-slim AS runtime

# Install runtime dependencies including cargo-expand
RUN apt-get update && apt-get install -y \
    git \
    curl \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Install cargo-expand (pinned version for Rust 1.85 compatibility)
RUN cargo install cargo-expand@1.0.105 --locked

WORKDIR /workspace

# Copy the binary
COPY --from=builder /build/target/release/rustbrain-ingestion /usr/local/bin/

# Create directory for mounted repos
RUN mkdir -p /workspace/target-repo

# Set environment
ENV RUST_LOG=info,rustbrain_ingestion=debug
ENV DATABASE_URL=postgresql://rustbrain:rustbrain_dev_2024@postgres:5432/rustbrain

# Default command
ENTRYPOINT ["rustbrain-ingestion"]
CMD ["--help"]
