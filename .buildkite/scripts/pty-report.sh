#!/usr/bin/env bash
# Script to help debug ENXIO errors from `openpty` while running macOS tests.
set -uo pipefail

echo "+++ :desktop_computer: ptys in use on the agent"
uptime  # indicates whether the VM is fresh

case $(uname -s) in
Darwin)
  echo "limit: $(sysctl -n kern.tty.ptmx_max)"
  echo "in use: $(find /dev -name 'ttys[0-9]*' | wc -l | tr -d ' ')"
  # Without root, lsof lists only the agent user's processes.
  pids=$(lsof -n -t /dev/ttys* 2>/dev/null | sort -un)
  ;;
Linux)
  echo "limit: $(cat /proc/sys/kernel/pty/max)"
  echo "in use: $(cat /proc/sys/kernel/pty/nr)"
  pids=$(find /proc/[0-9]*/fd -lname '/dev/pts/*' 2>/dev/null | cut -d/ -f3 | sort -un)
  ;;
*)
  echo "Not supported on $(uname -s)."
  exit 0
  ;;
esac

if [ -z "$pids" ]; then
  echo "No process holds a pty that we can see."
  exit 0
fi

# A comma-separated list, as `ps -p` expects on both macOS and Linux.
pids=$(printf '%s\n' "$pids" | paste -sd, -)
echo "Holders of ptys, by command:"
ps -o comm= -p "$pids" | sort | uniq -c | sort -rn | head -20
echo "Holders of ptys (the first 40 by pid):"
ps -o pid=,ppid=,etime=,tty=,command= -p "$pids" | sort -n | head -40
