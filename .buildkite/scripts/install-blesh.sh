#!/usr/bin/env bash
# Installs ble.sh where the e2e tests expect it (~/.local/share/blesh).
set -euo pipefail

version=0.4.0-devel3
mkdir -p "$HOME/.local/share"
curl -fsSL "https://github.com/akinomyoga/ble.sh/releases/download/v${version}/ble-${version}.tar.xz" |
  tar xJ -C "$HOME/.local/share"
rm -rf "$HOME/.local/share/blesh"
mv "$HOME/.local/share/ble-${version}" "$HOME/.local/share/blesh"
