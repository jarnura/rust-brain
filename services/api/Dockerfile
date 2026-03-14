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

# Copy manifests first for better caching
COPY Cargo.toml ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release && rm -rf src

# Copy actual source and build
COPY src ./src
RUN touch src/main.rs && cargo build --release

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
