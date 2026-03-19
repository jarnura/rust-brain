# =============================================================================
# rust-brain — API Service Dockerfile
# =============================================================================
# Tool API server for code intelligence queries
# =============================================================================

# Build stage
FROM rust:1.85-slim AS builder

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
COPY services/api/Cargo.toml ./services/api/
COPY services/ingestion/Cargo.toml ./services/ingestion/

# Create dummy source files for dependency caching
RUN mkdir -p crates/rustbrain-common/src \
    && mkdir -p services/api/src \
    && mkdir -p services/ingestion/src \
    && echo "pub fn dummy() {}" > crates/rustbrain-common/src/lib.rs \
    && echo "fn main() {}" > services/api/src/main.rs \
    && echo "fn main() {}" > services/ingestion/src/main.rs

# Build dependencies
RUN cargo build --release -p rustbrain-api

# Now copy actual source
COPY crates/rustbrain-common/src ./crates/rustbrain-common/src
COPY services/api/src ./services/api/src
COPY services/api/static ./services/api/static
COPY services/ingestion/src ./services/ingestion/src

# Touch to force rebuild
RUN touch crates/rustbrain-common/src/lib.rs \
    && touch services/api/src/main.rs

# Build the API service
RUN cargo build --release -p rustbrain-api

# Runtime stage
FROM debian:bookworm-slim AS runtime

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the binary
COPY --from=builder /build/target/release/rustbrain-api /usr/local/bin/

# Copy static files for playground
COPY --from=builder /build/services/api/static /app/static

# Create non-root user
RUN useradd -r -s /bin/false rustbrain && \
    chown -R rustbrain:rustbrain /app

USER rustbrain

# Set environment defaults
ENV RUST_LOG=info,rustbrain_api=debug
ENV API_PORT=8080

# Expose port
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

# Default command
ENTRYPOINT ["rustbrain-api"]
