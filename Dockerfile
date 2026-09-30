FROM rust:1.98.1-slim-bookworm AS builder

WORKDIR /app

COPY Cargo.toml rust-toolchain.toml ./
COPY src ./src

RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/snake-bollada /usr/local/bin/snake-bollada
COPY Rocket.toml /app/Rocket.toml

WORKDIR /app

ENV ROCKET_ADDRESS=0.0.0.0
ENV ROCKET_PORT=8000

EXPOSE 8000

CMD ["snake-bollada"]
