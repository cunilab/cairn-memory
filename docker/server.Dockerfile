# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY migrations migrations
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p cairn-server \
    && cp target/release/cairn-server /cairn-server

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home cairn
COPY --from=build /cairn-server /usr/local/bin/cairn-server
USER cairn
EXPOSE 3111
# Database URL comes from CAIRN_DATABASE_URL.
CMD ["cairn-server", "--bind", "0.0.0.0:3111"]
