FROM rust:1.99.0-bookworm
RUN rustup component add clippy rustfmt
RUN apt-get update && apt-get install -y --no-install-recommends libssl-dev pkg-config ca-certificates \
    && rm -rf /var/lib/apt/lists/*
ENV LANG=C.UTF-8
ENV CARGO_TARGET_DIR=/build/target
ENV RUSTUP_TOOLCHAIN=1.99.0
WORKDIR /workspace
