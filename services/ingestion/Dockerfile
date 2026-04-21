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
COPY services/validator/Cargo.toml ./services/validator/
COPY services/benchmarker/Cargo.toml ./services/benchmarker/
COPY services/audit/Cargo.toml ./services/audit/
COPY tests/fixtures/test-crate/Cargo.toml ./tests/fixtures/test-crate/

# Create dummy source files for dependency caching
RUN mkdir -p crates/rustbrain-common/src services/ingestion/src services/api/src \
             services/validator/src services/benchmarker/src services/audit/src \
             tests/fixtures/test-crate/src && \
    echo "pub fn dummy() {}" > crates/rustbrain-common/src/lib.rs && \
    echo "fn main() {}" > services/ingestion/src/main.rs && \
    echo "fn main() {}" > services/api/src/main.rs && \
    echo "fn main() {}" > services/validator/src/main.rs && \
    echo "fn main() {}" > services/benchmarker/src/main.rs && \
    echo "fn main() {}" > services/audit/src/main.rs && \
    echo "pub fn dummy() {}" > tests/fixtures/test-crate/src/lib.rs

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

# Create non-root user for security
RUN useradd --create-home --shell /bin/bash rustbrain

WORKDIR /workspace

# Copy the binary
COPY --from=builder /build/target/release/rustbrain-ingestion /usr/local/bin/

# Create directories for mounted repos and cache, set ownership for non-root user.
# CARGO_HOME (/usr/local/cargo) must be writable so cargo-expand can download
# and cache crate registries at runtime.
RUN mkdir -p /workspace/target-repo \
    && mkdir -p /tmp/rustbrain-expand-cache \
    && mkdir -p /tmp/cargo-target \
    && chown -R rustbrain:rustbrain /workspace /tmp/rustbrain-expand-cache /tmp/cargo-target /usr/local/cargo

# Set environment.
# CARGO_TARGET_DIR redirects build artifacts away from the (read-only) mounted
# source volume so cargo-expand can compile without write access to the repo.
ENV RUST_LOG=info,rustbrain_ingestion=debug
ENV CARGO_TARGET_DIR=/tmp/cargo-target
# DATABASE_URL must be passed at runtime via docker-compose or -e flag

# Switch to non-root user
USER rustbrain

# Default command
ENTRYPOINT ["rustbrain-ingestion"]
CMD ["--help"]
