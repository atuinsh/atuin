This directory contains vendored repositories.

Use [vendor.sh](vendor.sh) to manage vendored repositories:

```
Usage:
  vendor.sh add <repo-url> <ref> [name]
  vendor.sh list
  vendor.sh update <name> <ref>

<ref> specifies which branch/tag/commit to check out in the vendored
repository.

<name> is the name of the subdirectory in 'vendor/'. By default, it is
inferred from the repository URL.
```
