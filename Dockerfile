# syntax=docker/dockerfile:1
# fast-proxy container image (ADR 0033): a static musl binary on distroless, run as non-root.
#
# Local build, compiled from source:
#   docker build -t fast-proxy .
# The release workflow packages the binaries it already built instead:
#   docker buildx build --build-arg BINARY_SOURCE=prebuilt --platform linux/amd64,linux/arm64 .
# with dist/linux-amd64/fast-proxy and dist/linux-arm64/fast-proxy in the context.

# Which stage provides /fast-proxy: `source` (compile here) or `prebuilt` (from dist/).
ARG BINARY_SOURCE=source

# --- the admin UI, embedded into the binary from ui/dist ---
FROM --platform=$BUILDPLATFORM node:26-alpine AS ui
WORKDIR /src/ui
COPY ui/package.json ui/package-lock.json ./
RUN npm ci --ignore-scripts
COPY ui/ ./
RUN npm run build

# --- the binary, compiled for the image's platform (static: rustls and bundled SQLite) ---
FROM rust:1-alpine AS source
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
COPY --from=ui /src/ui/dist ui/dist
RUN cargo build --release --locked --bin fast-proxy \
    && cp target/release/fast-proxy /fast-proxy

# --- the binary, taken from the release build ---
FROM scratch AS prebuilt
ARG TARGETARCH
COPY dist/linux-${TARGETARCH}/fast-proxy /fast-proxy

FROM ${BINARY_SOURCE} AS binary

# --- an empty state directory, owned by the non-root user (distroless has no shell to make one) ---
FROM --platform=$BUILDPLATFORM busybox:1.37 AS layout
RUN mkdir -p /out/var/lib/fast-proxy

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=binary --chmod=0755 /fast-proxy /usr/local/bin/fast-proxy
COPY --from=layout --chown=65532:65532 /out/var/lib/fast-proxy /var/lib/fast-proxy
COPY packaging/docker/config.toml /etc/fast-proxy/config.toml
# config.toml is read from the working directory, so every subcommand finds it.
WORKDIR /etc/fast-proxy
USER 65532:65532
VOLUME ["/var/lib/fast-proxy"]
# 8000 plain HTTP, 8443 HTTPS (when [data_plane.tls] is enabled), 9000 the admin plane.
EXPOSE 8000 8443 9000
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/fast-proxy", "status", "--local"]
ENTRYPOINT ["/usr/local/bin/fast-proxy"]
