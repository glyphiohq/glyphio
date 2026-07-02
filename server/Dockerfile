# SPDX-License-Identifier: Apache-2.0
# Multi-stage build for the Glyphio sync server.
#
# Targets:
#   runtime — plain HTTP server for self-hosting (docker run / docker-compose).
#   lambda  — same binary + the AWS Lambda Web Adapter, for the serverless reference deploy.
#
# NOTE ON BUILD CONTEXT: the server depends on the sibling `src-tauri/crates/sync-proto` crate,
# so build from the REPOSITORY ROOT:
#   docker build -f server/Dockerfile --target runtime -t glyphio-server .
#   docker build -f server/Dockerfile --target lambda  -t glyphio-server-lambda .

FROM rust:1-slim-bookworm AS build
WORKDIR /src
# Only what the server needs: its own crate + the protocol crate it depends on.
COPY server/ server/
COPY src-tauri/crates/sync-proto/ src-tauri/crates/sync-proto/
RUN cargo build --release --manifest-path server/Cargo.toml

# ---- self-host runtime ----
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 glyphio \
    && mkdir -p /data && chown glyphio /data
COPY --from=build /src/server/target/release/glyphio-server /usr/local/bin/glyphio-server
USER glyphio
ENV PORT=8787 DB_PATH=/data/glyphio.db STORAGE=sqlite
EXPOSE 8787
VOLUME ["/data"]
ENTRYPOINT ["/usr/local/bin/glyphio-server"]

# ---- AWS Lambda (container image + web adapter) ----
FROM runtime AS lambda
COPY --from=public.ecr.aws/awsguru/aws-lambda-adapter:0.9.1 /lambda-adapter /opt/extensions/lambda-adapter
ENV AWS_LWA_PORT=8787 AWS_LWA_READINESS_CHECK_PATH=/healthz STORAGE=dynamo
