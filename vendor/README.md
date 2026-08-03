This directory contains vendored repositories.

Use [vendor.sh](vendor.sh) to manage vendored repositories:

```
Usage:
  vendor/vendor.sh add <repo-url> <ref> [name]
  vendor/vendor.sh update <name> <ref>
  vendor/vendor.sh list

<ref> specifies which branch/tag/commit to check out in the vendored
repository.

<name> is the name of the subdirectory in 'vendor/'. By default, it is
inferred from the repository URL.

Vendored repositories are recorded in 'vendor/.repositories.json'.
```

`add` and `update` stage their changes; review them and commit.
