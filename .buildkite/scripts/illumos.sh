#!/usr/bin/env bash
# Builds atuin-client and atuin-server on a real illumos (OmniOS) VM, as
# rust.yml's "Test in VM (illumos)" job did with vmactions/omnios-vm, which
# runs anyvm.py under the hood. Hosted Linux agents expose /dev/kvm, so the
# VM is KVM-accelerated. anyvm exits with the guest command's status.
set -euo pipefail

# The versions vmactions/omnios-vm pins for this release
# (conf/r151054-build.conf).
ANYVM_VERSION=0.6.5
ANYVM_SHA256=24fbcf739fc07d7fd655f493481cb77ed022b4baa81b039227080e1a7aaaa921
OMNIOS_RELEASE=r151054-build
BUILDER_VERSION=2.1.3
RUST_VERSION=1.98.0

# shellcheck source=lib.sh disable=SC1091
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

section ":package: Install QEMU" \
  apt_install qemu-system-x86 qemu-utils zstd xz-utils rsync openssh-client
section ":package: Fetch anyvm ${ANYVM_VERSION}" \
  fetch_verified "https://github.com/anyvm-org/anyvm/releases/download/v${ANYVM_VERSION}/anyvm.py" \
  "$ANYVM_SHA256" /tmp/anyvm.py

# Run inside the VM: the same steps as the old job's prepare + run.
guest_script="
set -e
cd /work
echo '--- :sunrise: Prepare OmniOS'
# pkg exits 4 when there is nothing to install.
pkg install pkg-config openssl || [ \$? -eq 4 ]
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
  sh -s -- -y --profile minimal --default-toolchain ${RUST_VERSION}
. \"\$HOME/.cargo/env\"
echo '--- :hammer: cargo build -p atuin-client --release'
cargo build -p atuin-client --locked --release
echo '--- :hammer: cargo build -p atuin-server --release'
cargo build -p atuin-server --locked --release
"

# The downloaded image lives in the `anyvm` cache; --snapshot runs from it
# without copying and discards the VM's disk writes.
echo "--- :sunrise: Boot OmniOS ${OMNIOS_RELEASE} and build"
python3 /tmp/anyvm.py \
  --os omnios --release "$OMNIOS_RELEASE" --builder "$BUILDER_VERSION" \
  --cpu "$(nproc)" --mem 16384 \
  --cache-dir "$HOME/.cache/anyvm" --data-dir /tmp/anyvm-data --snapshot \
  --vnc off \
  --sync rsync -v "$PWD:/work" \
  -- sh -c "$guest_script"
