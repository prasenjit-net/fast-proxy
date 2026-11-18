# Fuzzing

Fuzz targets for the pure functions that see untrusted input (M7 F1, ADR 0032), run with
[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer) on nightly Rust. This crate is
outside the Cargo workspace, so stable builds, `cargo test` and Windows never build it.

## Running

```sh
rustup toolchain install nightly
cargo install cargo-fuzz --locked --version 0.13.2

fuzz/run.sh 20                    # every target for 20 s, as CI does on each pull request
fuzz/run.sh 600 pipeline_resolve  # one target for 10 minutes
cd fuzz && cargo +nightly fuzz list
```

`run.sh` reads the checked-in `seeds/<target>/` and writes what it learns to `corpus/<target>/`, which
is ignored by git. A crash stops the run and leaves its input in `artifacts/<target>/`:

```sh
cd fuzz
cargo +nightly fuzz run <target> artifacts/<target>/crash-...   # reproduce
cargo +nightly fuzz tmin <target> artifacts/<target>/crash-...  # shrink it
```

CI runs every target for 20 s on each pull request (`Fuzz` job in `ci.yml`), and for 20 minutes each
every night (`fuzz-nightly.yml`), which opens an issue with the input when one fails. Before 1.0 each
target runs one CPU hour without a crash or timeout (M7 exit criterion 7).

## When something crashes

1. Reproduce and shrink the input as above.
2. Fix the function, not the target, unless the target's own assertion was wrong.
3. Commit the input as a unit test in the crate that owns the function (for example a
   `#[test]` in `crates/fast-proxy-model/src/cidr.rs` that parses the string), so the case is
   checked by `cargo test` on every platform, not only by the fuzzer.

## Targets

| Target | What it calls | What it asserts beyond "no panic" |
|---|---|---|
| `host_pattern` | `HostPattern::parse`, `HostTable`, `dns_host` | the written form parses back; an exact host finds itself |
| `route_prefix` | `RoutePrefix::parse`, `RouteTable::find` | the normal form parses back; the remainder is the rest of the path |
| `upstream_uri` | `build_upstream_uri` on the fixture's routes | the path starts with `/`, the authority is the endpoint, the query is kept |
| `forwarded_for` | `TrustedProxies::effective_client` | an untrusted peer is the client whatever it sends |
| `framing` | hop-by-hop removal, the 413/414 size checks | no hop-by-hop header or `Connection`-named header survives |
| `pipeline_resolve` | the pipeline's resolve step against a fixed snapshot | a forward's URI is well formed; a redirect is to `https://` |
| `config_toml` | `config.toml` parsing and its checks | |
| `model_json` | every admin write body, serde then its handler's validation | |
| `pem` | certificate, key and CA bundle parsing, upload validation | |
| `redactor` | the sampling redactor on headers, queries and bodies | a sensitive header never shows its value |
| `request_head_flags` | the sampler's unusual-head scan | |
| `acme_names` | `DomainAllowlist::parse`, `eligible_names` | a wildcard is never eligible (HTTP-01) |
| `cidr` | `Cidr::parse`, `contains` | the written form parses back |
| `gcra` | the rate limiter and the sampler's bucket, clock readings in any order | one instant gives a key at most its burst; a bucket that never refills gives at most what it holds |

The request-path targets run against `fast_proxy_fuzz::snapshot()`: exact and wildcard hosts, nested
prefixes with and without rewrites, a disabled route and subsystem, and a maintenance scope with a
bypass network. HTTPS redirects are not in it (they need a certificate store); their unit tests cover
them. Private functions are reached through `fuzzing` modules in `fast-proxy-dataplane` and
`fast-proxy-admin`, compiled only with their `fuzzing` feature, so none of them is public API. The
binary's `config.toml` model is included by path, since the binary has no library target.

Inputs that are text (`host_pattern`, `pem`, `cidr`) split their parts at the first NUL byte, so seeds
can be written by hand. Structured inputs pick hosts and header names the request path cares about
from short lists most of the time, so the fuzzer reaches routing instead of stopping at an unknown
host. The `pem` seeds hold certificates only: a private key in the repository, even a throwaway one,
would trip secret scanners.
