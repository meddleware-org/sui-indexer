# ── Build stage ───────────────────────────────────────────────────────────────
# Bases pinned by digest for reproducible builds; the tags are kept for readability.
# To bump: skopeo inspect --format '{{.Digest}}' docker://docker.io/library/node:24-trixie-slim
FROM docker.io/library/node:25-trixie-slim@sha256:aabbe39553d15ede8a97cc60c9e1a97034ff772afcf696ea42b94e7f5f2ec71b AS builder

WORKDIR /src
COPY package.json package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY tsconfig.json ./
COPY src ./src
COPY scripts ./scripts
# One self-contained ESM bundle: the runtime image needs no node_modules.
RUN npm run build && npm run licenses

# ── Runtime stage ─────────────────────────────────────────────────────────────
# distroless Node 24: no shell, no package manager; runs as nonroot (65532).
# To bump: skopeo inspect --format '{{.Digest}}' docker://gcr.io/distroless/nodejs24-debian13:nonroot
FROM gcr.io/distroless/nodejs24-debian13:nonroot@sha256:9eeb7f5887d0e239e78264b06f7f11d2e14be534050481803a9e4728fcdd278e

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
COPY --from=builder /src/dist/main.mjs ./main.mjs
# The bundle carries no package metadata, so ship what an image scanner and a licence audit need:
# the lockfile (so SBOM tools see the bundled npm packages) and the third-party licence texts.
COPY --from=builder /src/package.json /src/package-lock.json ./
COPY --from=builder /src/dist/THIRD_PARTY_LICENSES ./THIRD_PARTY_LICENSES

USER 65532:65532
EXPOSE 8080
ENV PORT=8080 DB_PATH=/data/indexer.db
VOLUME ["/data"]

# The entrypoint is /nodejs/bin/node. node:sqlite is stable in use but still flagged experimental.
CMD ["--disable-warning=ExperimentalWarning", "/app/main.mjs"]
