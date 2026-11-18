#!/usr/bin/env bash
# Runs fuzz targets for a while each (M7 F2): `fuzz/run.sh SECONDS [TARGET...]`,
# every target when none is named. New inputs go to the ignored corpus/; the
# checked-in seeds/ are only read. Exits non-zero on the first crash, which
# cargo-fuzz leaves in artifacts/<target>/.
set -euo pipefail

cd "$(dirname "$0")"
seconds="${1:?usage: fuzz/run.sh SECONDS [TARGET...]}"
shift
targets=()
if [ "$#" -gt 0 ]; then
  targets=("$@")
else
  while IFS= read -r target; do targets+=("${target}"); done < <(cargo +nightly fuzz list)
fi

cargo +nightly fuzz build
for target in "${targets[@]}"; do
  echo "== ${target} for ${seconds}s"
  mkdir -p "corpus/${target}"
  seeds="seeds/${target}"
  [ -d "${seeds}" ] || seeds="corpus/${target}"
  cargo +nightly fuzz run "${target}" "corpus/${target}" "${seeds}" -- \
    -max_total_time="${seconds}" -timeout=10 -rss_limit_mb=2048
done
