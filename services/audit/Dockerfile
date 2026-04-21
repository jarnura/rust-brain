# =============================================================================
# rust-brain — Audit Service Dockerfile
# =============================================================================
# Workspace leak detection and compliance scanning service
# =============================================================================

# Build stage
FROM rust:1.88-slim AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy workspace manifest
COPY Cargo.toml ./

# Copy crate manifests
COPY crates/rustbrain-common/Cargo.toml ./crates/rustbrain-common/
COPY services/audit/Cargo.toml ./services/audit/
COPY services/api/Cargo.toml ./services/api/
COPY services/ingestion/Cargo.toml ./services/ingestion/
COPY services/validator/Cargo.toml ./services/validator/
COPY services/benchmarker/Cargo.toml ./services/benchmarker/
COPY tests/fixtures/test-crate/Cargo.toml ./tests/fixtures/test-crate/

# Create dummy source files for dependency caching
RUN mkdir -p crates/rustbrain-common/src \
    && mkdir -p services/audit/src \
    && mkdir -p services/api/src \
    && mkdir -p services/ingestion/src \
    && mkdir -p services/validator/src \
    && mkdir -p services/benchmarker/src \
    && mkdir -p tests/fixtures/test-crate/src \
    && echo "pub fn dummy() {}" > crates/rustbrain-common/src/lib.rs \
    && echo "fn main() {}" > services/audit/src/main.rs \
    && echo "fn main() {}" > services/api/src/main.rs \
    && echo "fn main() {}" > services/ingestion/src/main.rs \
    && echo "fn main() {}" > services/validator/src/main.rs \
    && echo "fn main() {}" > services/benchmarker/src/main.rs \
    && echo "pub fn dummy() {}" > tests/fixtures/test-crate/src/lib.rs

# Build dependencies
RUN cargo build --release -p rustbrain-audit

# Now copy actual source
COPY crates/rustbrain-common/src ./crates/rustbrain-common/src
COPY services/audit/src ./services/audit/src

# Touch to force rebuild
RUN touch crates/rustbrain-common/src/lib.rs \
    && touch services/audit/src/main.rs

# Build the audit service
RUN cargo build --release -p rustbrain-audit

# Docker CLI stage — copy only the binary, no daemon
FROM docker:27-cli AS docker-cli

# Runtime stage
FROM debian:bookworm-slim AS runtime

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the Docker CLI binary (needed for orphan volume/container detection)
COPY --from=docker-cli /usr/local/bin/docker /usr/local/bin/docker

# Copy the binary
COPY --from=builder /build/target/release/rustbrain-audit /usr/local/bin/

# Create non-root user with Docker socket access.
# GID matches the host docker group that owns /var/run/docker.sock.
RUN groupadd -g 125 docker && \
    useradd -r -s /bin/false -G docker rustbrain && \
    chown -R rustbrain:rustbrain /app

USER rustbrain

# Set environment defaults
ENV RUST_LOG=info,rustbrain_audit=debug
ENV AUDIT_PORT=8090

# Expose port
EXPOSE 8090

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8090/health || exit 1

# Default command
ENTRYPOINT ["rustbrain-audit"]
