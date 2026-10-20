# =============================================================================
# sui-indexer — one image, two binaries: /app/ingest (the sole writer) and /app/api (read-only).
# =============================================================================
# Multi-stage build with cargo-chef for dependency-layer caching:
#   chef     — the Rust toolchain plus cargo-chef
#   planner  — computes the dependency recipe from Cargo.toml/lock
#   notices  — cargo-about licence notices of each binary (fails on an unaccepted licence)
#   builder  — compiles the dependencies (cached), then the two binaries (--locked)
#   runtime  — distroless/cc: glibc + CA certificates; no shell, no package manager
#
# Storage is PostgreSQL over rustls (ring, the same provider as the gRPC client; the database CA is
# mounted at run time), so the runtime needs only glibc. The 0.0.14 image is built with the temporary
# `import-sqlite` feature (CARGO_FEATURES below), which compiles SQLite in for `ingest import-sqlite`;
# 0.0.15 removes the feature, and with it SQLite and this argument's default. Both base images are
# pinned by digest (multi-arch index); bump tag and digest together
# (`skopeo inspect --format '{{.Digest}}' docker://<image>:<tag>`).
# =============================================================================

# The feature set the shipped binaries are built with (global default; the stages that use it redeclare it).
# REMOVE in 0.0.15 together with the importer.
ARG CARGO_FEATURES="sui-indexer-ingest/import-sqlite"

FROM docker.io/library/rust:1-slim@sha256:4df2ae8cfdfebdfc13a65da7f253b73d68983dc0339f2719bc17a279c913b13d AS chef
RUN cargo install cargo-chef --version 0.1.78 --locked
WORKDIR /src

# ── Planner: derive the dependency recipe ────────────────────────────────────
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Licence notices (Apache-2.0 §4(a) and the MIT/BSD notice clauses) ────────
# `--fail` stops the build when a linked crate's licence is not accepted by about.toml. CI builds this
# target alone (`--target notices`) so a licence problem shows up without a full compile.
FROM chef AS notices
ARG CARGO_FEATURES
RUN cargo install --locked --version 0.9.2 --features cli cargo-about
COPY . .
RUN mkdir -p /out \
 && for bin in api ingest; do \
      feat=""; [ "$bin" = ingest ] && [ -n "${CARGO_FEATURES}" ] && feat="--features import-sqlite"; \
      cargo about generate --locked --fail -c about.toml --manifest-path "crates/${bin}/Cargo.toml" ${feat} about.hbs \
        > "/out/THIRD_PARTY_LICENSES-${bin}.txt"; \
    done \
 && grep -q 'Apache License' /out/THIRD_PARTY_LICENSES-ingest.txt

# ── Builder: compile deps (cached), then the binaries ────────────────────────
# Two cargo invocations, so a feature of one binary (the ingester's temporary SQLite importer) never reaches
# the other (cargo unifies features within one build): the API image layer never links SQLite.
FROM chef AS builder
ARG CARGO_FEATURES
COPY --from=planner /src/recipe.json recipe.json
# Re-used on every build where only the sources change (not Cargo.lock).
RUN cargo chef cook --release --locked --recipe-path recipe.json -p sui-indexer-api \
 && cargo chef cook --release --locked --recipe-path recipe.json -p sui-indexer-ingest ${CARGO_FEATURES:+--features "${CARGO_FEATURES}"}
COPY . .
RUN cargo build --release --locked -p sui-indexer-api \
 && cargo build --release --locked -p sui-indexer-ingest ${CARGO_FEATURES:+--features "${CARGO_FEATURES}"} \
 && install -D -m 0755 target/release/api /out/api \
 && install -D -m 0755 target/release/ingest /out/ingest

# ── Runtime: minimal distroless image ────────────────────────────────────────
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2

ARG VERSION=dev
ARG VENDOR="Meddleware"
ARG DESCRIPTION="Lean read-indexer for the Meddleware Sui packages (access_gate, sealed_content)."
ARG SOURCE_URL="https://github.com/meddleware-org/sui-indexer"
ARG DOCUMENTATION_URL="https://github.com/meddleware-org/sui-indexer#readme"
ARG IMAGE_URL="https://quay.io/meddleware-org/sui-indexer"

LABEL org.opencontainers.image.title="sui-indexer" \
      org.opencontainers.image.description="${DESCRIPTION}" \
      org.opencontainers.image.url="${IMAGE_URL}" \
      org.opencontainers.image.source="${SOURCE_URL}" \
      org.opencontainers.image.documentation="${DOCUMENTATION_URL}" \
      org.opencontainers.image.vendor="${VENDOR}" \
      org.opencontainers.image.licenses="0BSD" \
      org.opencontainers.image.version="${VERSION}"

WORKDIR /app
COPY --from=builder /out/api /out/ingest /app/
COPY --from=notices /out/ /app/
# The lockfile makes an image scan (Trivy, syft) list the compiled crates (a stripped binary carries
# none); it is inert at runtime. The release also attests a Cargo SBOM of the two binaries.
COPY Cargo.lock /app/Cargo.lock
COPY LICENSE /app/LICENSE

# 65532 is distroless's nonroot user (the pod's runAsUser and the volume's fsGroup).
USER 65532:65532
EXPOSE 8080
ENV PORT=8080

# The API by default; the ingester runs as `/app/ingest` (Kubernetes `command:`), and
# `/app/ingest --healthcheck` is its exec liveness probe. No volume: all state is in PostgreSQL
# (DATABASE_URL, with the CA at /etc/postgres-ca/ca.crt).
CMD ["/app/api"]
