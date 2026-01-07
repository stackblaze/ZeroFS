FROM rust:1.83-bullseye AS builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    ca-certificates \
    make \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src

COPY zerofs/Cargo.toml zerofs/Cargo.lock zerofs/build.rs ./zerofs/
COPY zerofs/src ./zerofs/src
COPY zerofs/proto ./zerofs/proto

WORKDIR /usr/src/zerofs

RUN cargo build --release

FROM debian:bullseye-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl1.1 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/zerofs/target/release/zerofs /usr/local/bin/zerofs

RUN useradd -m -u 1001 zerofs
USER zerofs

# Default ports that might be used - actual configuration comes from TOML file
EXPOSE 2049 5564 10809

ENTRYPOINT ["zerofs"]
