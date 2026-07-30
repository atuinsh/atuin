#!/usr/bin/env bash
# Enforces the native-tls migration invariant on the resolved dependency graph:
#   - the native-tls stack MUST be present
#   - the rustls stack MUST be absent
# Reads Cargo.lock (arg 1, default ./Cargo.lock). Matches only [[package]]
# `name = "..."` declarations, so transitive version-pin lines never false-match.
set -euo pipefail

LOCK="${1:-Cargo.lock}"

if [[ ! -f "$LOCK" ]]; then
  echo "check-tls-backend: lockfile not found: $LOCK" >&2
  exit 2
fi

fail=0

forbidden=(rustls hyper-rustls tokio-rustls)
for crate in "${forbidden[@]}"; do
  if grep -q "^name = \"${crate}\"$" "$LOCK"; then
    echo "FAIL: forbidden rustls-stack crate present: ${crate}"
    fail=1
  fi
done

required=(native-tls openssl-sys)
for crate in "${required[@]}"; do
  if ! grep -q "^name = \"${crate}\"$" "$LOCK"; then
    echo "FAIL: expected native-tls-stack crate missing: ${crate}"
    fail=1
  fi
done

if [[ "$fail" -ne 0 ]]; then
  echo "TLS backend check FAILED"
  exit 1
fi
echo "TLS backend check OK: native-tls present, rustls absent"
