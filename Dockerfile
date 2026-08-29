# syntax=docker/dockerfile:1

FROM rust:1.88-bookworm AS builder
WORKDIR /app

# Copy manifests first so dependency compilation can be cached between builds.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home aframp

COPY --from=builder /app/target/release/aframp /usr/local/bin/aframp
USER aframp

EXPOSE 3000
CMD ["/usr/local/bin/aframp"]
