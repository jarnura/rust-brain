FROM rust:1.88-slim AS builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .
RUN cargo build --release -p rustbrain-mcp --features sse

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/rustbrain-mcp /usr/local/bin/
HEALTHCHECK CMD curl -f http://localhost:3001/health || exit 1
EXPOSE 3001
CMD ["rustbrain-mcp", "--transport", "sse", "--port", "3001", "--api-base-url", "http://api:8080"]
