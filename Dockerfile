FROM rust:1.98-bookworm AS chef
RUN apt-get update && apt-get install -y --no-install-recommends libssl-dev pkg-config && rm -rf /var/lib/apt/lists/*
RUN rustup component add clippy && cargo install cargo-chef --version 0.1.74 --locked
WORKDIR /build

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=planner /build/recipe.json ./recipe.json
# This layer changes only when manifests or Cargo.lock change.
RUN cargo chef cook --locked --release --recipe-path recipe.json -p ultrafinance-api -p ultrafinance-cli
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY website ./website
RUN cargo build --locked --release -p ultrafinance-api -p ultrafinance-cli

FROM build AS tested
COPY data/merchants.example.json ./data/merchants.example.json
COPY data/locations ./data/locations
COPY evals/location-smoke.json ./evals/location-smoke.json
RUN cargo test --locked --release --workspace && cargo clippy --locked --release --workspace --all-targets -- -D warnings

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*
COPY deploy/rds-ca/ /usr/local/share/ca-certificates/ultrafinance-rds/
RUN update-ca-certificates
COPY --from=public.ecr.aws/awsguru/aws-lambda-adapter:1.1.0 /lambda-adapter /opt/extensions/lambda-adapter
COPY --from=tested /build/target/release/ultrafinance-api /usr/local/bin/ultrafinance-api
COPY --from=tested /build/target/release/ultrafinance /usr/local/bin/ultrafinance
COPY deploy/entrypoint.sh /usr/local/bin/entrypoint
ENV ULTRAFINANCE_BIND=0.0.0.0:8080 ULTRAFINANCE_REQUIRE_POSTGRES=true AWS_LWA_PORT=8080 AWS_LWA_READINESS_CHECK_PATH=/health
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/entrypoint"]
