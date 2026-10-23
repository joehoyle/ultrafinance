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
COPY deploy/rds-ca ./deploy/rds-ca
ARG ULTRAFINANCE_BUILD_TAG=""
ARG ULTRAFINANCE_BUILD_REVISION=""
ARG ULTRAFINANCE_BUILD_DIRTY="false"
ARG ULTRAFINANCE_BUILD_TIME=""
ENV ULTRAFINANCE_BUILD_TAG=$ULTRAFINANCE_BUILD_TAG \
    ULTRAFINANCE_BUILD_REVISION=$ULTRAFINANCE_BUILD_REVISION \
    ULTRAFINANCE_BUILD_DIRTY=$ULTRAFINANCE_BUILD_DIRTY \
    ULTRAFINANCE_BUILD_TIME=$ULTRAFINANCE_BUILD_TIME
RUN cargo build --locked --release -p ultrafinance-api -p ultrafinance-cli

FROM debian:bookworm-slim AS duckdb
# The production CLI needs DuckDB and cached extensions in its HTTPS-only VPC.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl unzip && rm -rf /var/lib/apt/lists/*
RUN curl -fLsS https://github.com/duckdb/duckdb/releases/download/v1.5.6/duckdb_cli-linux-arm64.zip -o /tmp/duckdb.zip \
    && echo 'c544e92c9b7c31fc53c2139802cabd8e2d1b2b3e3f933117f31611239c1402db  /tmp/duckdb.zip' | sha256sum -c - \
    && unzip -q /tmp/duckdb.zip -d /usr/local/bin \
    && chmod +x /usr/local/bin/duckdb \
    && rm /tmp/duckdb.zip
# Fetch signed extension files over HTTPS; DuckDB's HTTPS bootstrap otherwise
# tries to install httpfs over HTTP before it can use HTTPS repositories.
RUN set -eu; for extension in httpfs iceberg; do \
    curl -fLsS "https://extensions.duckdb.org/v1.5.6/linux_arm64/$extension.duckdb_extension.gz" -o "/tmp/$extension.duckdb_extension.gz"; \
    gzip -d "/tmp/$extension.duckdb_extension.gz"; \
    duckdb -batch :memory: -c "INSTALL '/tmp/$extension.duckdb_extension';"; \
    rm "/tmp/$extension.duckdb_extension"; \
    done
RUN duckdb -batch :memory: -c 'LOAD httpfs; LOAD iceberg; SELECT 1;'

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends awscli bash ca-certificates curl libssl3 tmux \
    && sed -i 's|http://|https://|g' /etc/apt/sources.list.d/debian.sources \
    && rm -rf /var/lib/apt/lists/*
# Match Aurora 17 and local directory archives; Debian bookworm ships client 15.
RUN curl -fsS https://www.postgresql.org/media/keys/ACCC4CF8.asc -o /usr/share/keyrings/postgresql.asc \
    && echo 'deb [signed-by=/usr/share/keyrings/postgresql.asc] https://apt.postgresql.org/pub/repos/apt bookworm-pgdg main' > /etc/apt/sources.list.d/pgdg.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends postgresql-client-17 \
    && rm -rf /var/lib/apt/lists/*
COPY deploy/bashrc /etc/bash.bashrc
COPY deploy/tmux.conf /etc/tmux.conf
COPY --from=duckdb /usr/local/bin/duckdb /usr/local/bin/duckdb
COPY --from=duckdb /root/.duckdb/ /root/.duckdb/
ENV SHELL=/bin/bash
COPY deploy/rds-ca/ /usr/local/share/ca-certificates/ultrafinance-rds/
RUN update-ca-certificates
COPY --from=public.ecr.aws/awsguru/aws-lambda-adapter:1.1.0 /lambda-adapter /opt/extensions/lambda-adapter
COPY --from=build /build/target/release/ultrafinance-api /usr/local/bin/ultrafinance-api
COPY --from=build /build/target/release/ultrafinance /usr/local/bin/ultrafinance
COPY deploy/entrypoint.sh /usr/local/bin/entrypoint
ENV ULTRAFINANCE_BIND=0.0.0.0:8080 ULTRAFINANCE_REQUIRE_POSTGRES=true AWS_LWA_PORT=8080 AWS_LWA_READINESS_CHECK_PATH=/health
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/entrypoint"]
