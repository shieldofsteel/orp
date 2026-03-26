# ── Stage 1: Builder ──────────────────────────────────────────────────────────
FROM rust:1.75-bookworm AS builder

WORKDIR /build

# Install build dependencies
RUN apt-get update -q && apt-get install -y --no-install-recommends \
    protobuf-compiler \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Cache dependencies layer — copy manifests first
COPY Cargo.toml Cargo.lock ./
COPY crates/orp-core/Cargo.toml         crates/orp-core/Cargo.toml
COPY crates/orp-storage/Cargo.toml      crates/orp-storage/Cargo.toml
COPY crates/orp-stream/Cargo.toml       crates/orp-stream/Cargo.toml
COPY crates/orp-connector/Cargo.toml    crates/orp-connector/Cargo.toml
COPY crates/orp-query/Cargo.toml        crates/orp-query/Cargo.toml
COPY crates/orp-entity/Cargo.toml       crates/orp-entity/Cargo.toml
COPY crates/orp-config/Cargo.toml       crates/orp-config/Cargo.toml
COPY crates/orp-audit/Cargo.toml        crates/orp-audit/Cargo.toml
COPY crates/orp-proto/Cargo.toml        crates/orp-proto/Cargo.toml
COPY crates/orp-geospatial/Cargo.toml   crates/orp-geospatial/Cargo.toml
COPY crates/orp-security/Cargo.toml     crates/orp-security/Cargo.toml
COPY crates/orp-testbed/Cargo.toml      crates/orp-testbed/Cargo.toml

# Create stub src dirs so cargo can resolve the workspace without full source
RUN for crate in crates/*/; do \
      mkdir -p "$crate/src" && \
      echo 'fn main() {}' > "$crate/src/main.rs" && \
      echo '' > "$crate/src/lib.rs"; \
    done

# Pre-fetch + compile dependencies only
RUN cargo build --release -p orp-core 2>&1 | tail -5 || true

# Now copy real source
COPY . .

# Touch to force rebuild of our crates (not deps)
RUN find crates -name "*.rs" -exec touch {} +

# Final release build — lto=thin + strip=true already set in Cargo.toml profile
RUN cargo build --release -p orp-core

# Verify the binary exists
RUN ls -lh target/release/orp

# ── Stage 2: Minimal runtime ──────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

# Runtime deps only — no build tools
RUN apt-get update -q && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Non-root user for security
RUN useradd -r -u 1001 -s /sbin/nologin orp

# Copy the stripped binary from builder
COPY --from=builder /build/target/release/orp /usr/local/bin/orp

# Ensure binary is executable
RUN chmod +x /usr/local/bin/orp

# Data directory
RUN mkdir -p /var/lib/orp /etc/orp && chown -R orp:orp /var/lib/orp /etc/orp

USER orp

WORKDIR /var/lib/orp

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD orp status || exit 1

EXPOSE 9090

ENTRYPOINT ["orp", "start"]
