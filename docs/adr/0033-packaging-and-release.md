# ADR 0033: packaging and release

Status: accepted (2026-11-16, the owner). Plan reference: M7 L15, tasks PK1 to
PK5.

## Context

The release workflow builds four binaries: Linux x86_64 (glibc), macOS arm64 and x86_64, and
Windows x86_64. There is no container image, no checksum file, no SBOM and no build provenance.

## Decision

- **Static Linux binaries.** Add `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`.
  rustls and bundled SQLite need no system library, so the binaries are fully static. The four
  existing targets stay.
- **Container image.** A multi-stage build copies the musl binary into
  `gcr.io/distroless/static:nonroot`: no shell, and it runs as user 65532.
  - `/var/lib/fast-proxy` is the volume for the database and the master key; the default config
    points there.
  - Ports 8000, 8443 and 9000 are exposed. The admin plane stays on loopback unless admin TLS is
    configured, as the recipe shows.
  - The health check runs `fast-proxy status --local`, a new flag that calls `/api/health` with no
    token.
  - Published multi-arch (amd64, arm64) to **Docker Hub** as `<namespace>/fast-proxy:<version>`,
    `:<major>.<minor>` and `:latest` by the release workflow (changed 2026-11-18 from GHCR, the owner's
    request). The image packages the musl binaries the release already built (`BINARY_SOURCE=prebuilt`),
    so arm64 is never compiled under emulation; `docker build .` compiles from source. Before pushing,
    the workflow smoke-tests the amd64 image (non-root user, `status --local`, the version, one 404).
- **Homebrew** (added 2026-11-18, the owner's request; was after 1.0). The release writes
  `Formula/fast-proxy.rb` to a tap repository (default `<owner>/homebrew-tap`) with
  `scripts/homebrew-formula.sh`: the Apple binaries on macOS, the static musl ones on Linux, checksums
  from the release's `.sha256` files, and a `brew services` definition that keeps state under
  `var/lib/fast-proxy`.
- **Integrity.** Every release carries `SHA256SUMS`, a CycloneDX SBOM (`cargo cyclonedx`) and
  GitHub build provenance. The image is signed with keyless cosign. The workflow's token
  permissions are the narrowest that work.
- **Smoke test.** CI builds the image on every pull request and checks four things: it starts,
  `/api/health` answers, one request is proxied, and the process runs as non-root.
- **Changelog.** `CHANGELOG.md` (Keep a Changelog), from v0.1.1 on, fills the GitHub release
  notes.

## Consequences

Two more release jobs, an image job and a formula job. The image and the formula are published after
the GitHub Release, so a failure there leaves the release in place and the job can be re-run. The
release needs a Docker Hub account and token and a tap repository with a token that can push to it
(README, "CI & releases"). Package-manager packages (deb, rpm) and a Helm chart stay after 1.0.
