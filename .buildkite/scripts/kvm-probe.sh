#!/usr/bin/env bash
# TEMPORARY: checks whether hosted Linux agents can run KVM-accelerated VMs
# (what the illumos VM job needs). Remove after the probe.
set -uo pipefail

echo "--- :computer: Host"
uname -a
echo "vCPUs: $(nproc)"
grep -m1 'model name' /proc/cpuinfo
echo "CPU virtualization flags: $(grep -o -w -E 'vmx|svm' /proc/cpuinfo | sort -u | tr '\n' ' ')"
echo "running under a hypervisor: $(grep -q -w hypervisor /proc/cpuinfo && echo yes || echo no)"
command -v systemd-detect-virt >/dev/null && echo "systemd-detect-virt: $(systemd-detect-virt 2>&1)"

echo "--- :key: /dev/kvm"
ls -l /dev/kvm 2>&1
lsmod 2>/dev/null | grep -E '^kvm' || echo "(no kvm modules listed by lsmod)"
python3 - <<'PY'
import fcntl
import os

KVM_GET_API_VERSION = 0xAE00
KVM_CREATE_VM = 0xAE01
try:
    kvm = os.open("/dev/kvm", os.O_RDWR)
except OSError as err:
    print(f"RESULT: no usable KVM (open /dev/kvm failed: {err})")
    raise SystemExit(0)
print("KVM_GET_API_VERSION:", fcntl.ioctl(kvm, KVM_GET_API_VERSION))
try:
    vm = fcntl.ioctl(kvm, KVM_CREATE_VM, 0)
    print(f"RESULT: KVM works (KVM_CREATE_VM returned fd {vm})")
except OSError as err:
    print(f"RESULT: /dev/kvm opens but KVM_CREATE_VM failed: {err}")
PY
