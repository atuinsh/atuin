"""Test harness for atuin end-to-end tests.

Modules:
  images     -- build the base + per-shell docker images from the repo source
  containers -- testcontainers wrappers for the sync server and shell clients
  session    -- PtyShell: drive a shell in a container over a PTY, parse with pyte
  atuin      -- high-level actions (register, login, sync, search) + shell adapters
"""
