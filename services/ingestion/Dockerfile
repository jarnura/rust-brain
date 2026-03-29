# =============================================================================
# rust-brain — Ingestion Service Dockerfile
# =============================================================================
# Macro expansion and source ingestion service
# =============================================================================

# Build stage
FROM rust:slim AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy workspace manifests first for better caching
COPY Cargo.toml ./
COPY crates/rustbrain-common/Cargo.toml ./crates/rustbrain-common/
COPY services/ingestion/Cargo.toml ./services/ingestion/
COPY services/api/Cargo.toml ./services/api/

# Create dummy source files for dependency caching
RUN mkdir -p crates/rustbrain-common/src services/ingestion/src services/api/src && \
    echo "pub fn dummy() {}" > crates/rustbrain-common/src/lib.rs && \
    echo "fn main() {}" > services/ingestion/src/main.rs && \
    echo "fn main() {}" > services/api/src/main.rs

# Build dependencies only (keep manifests for later)
RUN cargo build --release -p rustbrain-ingestion

# Copy actual source (overwrite dummy files)
COPY crates/rustbrain-common/src ./crates/rustbrain-common/src
COPY services/ingestion/src ./services/ingestion/src

# Build for real
RUN touch crates/rustbrain-common/src/lib.rs services/ingestion/src/main.rs && \
    cargo build --release -p rustbrain-ingestion

# Test stage - skipped due to unstable feature requirements
# FROM builder AS tester
# WORKDIR /build
# RUN cargo test --release -p rustbrain-ingestion --all

# Runtime stage
FROM rust:slim AS runtime

# Install runtime dependencies including cargo-expand
RUN apt-get update && apt-get install -y \
    git \
    curl \
    pkg-config \
    libssl-dev \
    make \
    protobuf-compiler \
    g++ \
    python3 \
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
# DATABASE_URL must be passed at runtime via docker-compose or -e flag

# Default command
ENTRYPOINT ["rustbrain-ingestion"]
CMD ["--help"]
