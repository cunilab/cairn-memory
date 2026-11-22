# syntax=docker/dockerfile:1
FROM rust:1-trixie AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY migrations migrations
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p cairn-server \
    && cp target/release/cairn-server /cairn-server

FROM python:3.13-slim-bookworm AS embedding-model
WORKDIR /provision
COPY crates/server/assets/minilm crates/server/assets/minilm
COPY scripts/download_embedding_model.py scripts/download_embedding_model.py
RUN python3 scripts/download_embedding_model.py /model

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home cairn
COPY --from=build /cairn-server /usr/local/bin/cairn-server
COPY --from=embedding-model /model /opt/cairn/models/minilm
COPY crates/server/assets/minilm/LICENSE crates/server/assets/minilm/README.md /opt/cairn/models/
ENV CAIRN_EMBEDDING_MODEL_DIR=/opt/cairn/models/minilm
USER cairn
EXPOSE 3111
# Database URL comes from CAIRN_DATABASE_URL.
CMD ["cairn-server", "--bind", "0.0.0.0:3111"]
