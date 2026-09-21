# Known Issues

- SQLite has some issues with ZFS in certain configurations. As Atuin uses SQLite, this may cause your shell to become slow! We've got an [issue](https://github.com/atuinsh/atuin/issues/952) to track, with some workarounds
- SQLite also doesn't tend to like network filesystems (for example, NFS)
