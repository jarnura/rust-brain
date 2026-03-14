# =============================================================================
# rust-brain — MCP Server Dockerfile
# =============================================================================
# MCP server for code intelligence tools
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
COPY --from=builder /build/target/release/rustbrain-mcp /usr/local/bin/

# Create non-root user
RUN useradd -r -s /bin/false rustbrain && \
    chown -R rustbrain:rustbrain /app

USER rustbrain

# Set environment defaults
ENV RUST_LOG=info,rustbrain_mcp=debug
ENV API_BASE_URL=http://rustbrain-api:8080

# Health check (MCP server uses stdio, so this is just for container health)
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

# Default command (stdio mode)
ENTRYPOINT ["rustbrain-mcp"]
CMD ["--transport", "stdio"]
