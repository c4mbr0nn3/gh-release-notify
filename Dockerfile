# --- builder ---
FROM rust:1.86-slim AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Cache deps: copy manifests first, build deps, then copy source.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && cargo build --release && rm -rf src

COPY src ./src
RUN touch src/main.rs && cargo build --release

# --- runtime ---
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 ghrel

COPY --from=builder /build/target/release/gh-release-notify /usr/local/bin/gh-release-notify

USER ghrel
WORKDIR /home/ghrel

ENTRYPOINT ["gh-release-notify"]
CMD ["--config", "/config/config.toml"]