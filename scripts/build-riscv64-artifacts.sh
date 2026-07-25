#!/usr/bin/env bash
# Builds riscv64 release tarballs as dist "extra artifacts" (see
# dist-workspace.toml), cross-compiled with cargo-zigbuild.
#
# riscv64 can't be a first-class dist target: with install-updater = true,
# dist requires a prebuilt axoupdater binary for every target, and axoupdater
# publishes none for riscv64. install-updater is all-or-nothing, and turning
# it off would break `atuin update` everywhere. Fold this back into
# [dist].targets once axoupdater ships riscv64 builds.
set -euo pipefail

TARGET=riscv64gc-unknown-linux-gnu

rustup target add "$TARGET"
if ! command -v cargo-zigbuild >/dev/null 2>&1; then
  pip3 install cargo-zigbuild 2>/dev/null || pip3 install --break-system-packages cargo-zigbuild
  export PATH="$HOME/.local/bin:$PATH"
fi

cargo zigbuild --profile dist --target "$TARGET" -p atuin -p atuin-server

for bin in atuin atuin-server; do
  dir="${bin}-${TARGET}"
  rm -rf "$dir"
  mkdir "$dir"
  cp "target/${TARGET}/dist/${bin}" CHANGELOG.md LICENSE README.md "$dir/"
  tar czf "${dir}.tar.gz" "$dir"
  rm -rf "$dir"
  # same format dist emits: "<sha256> *<file>"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -b "${dir}.tar.gz" > "${dir}.tar.gz.sha256"
  else
    shasum -a 256 -b "${dir}.tar.gz" > "${dir}.tar.gz.sha256"
  fi
done
