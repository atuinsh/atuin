Testing `bash-preexec`
======================

**Note on test conditions**

When writing test conditions, use `[ ... ]` instead of `[[ ... ]]` since the
former are supported by Bats on Bash versions before 4.1. In particular, macOS
uses Bash 3.2, and `[[ ... ]]` tests always pass on macOS.

In some cases, you may want to use a feature unique to `[[ ... ]]` such as
pattern matching (`[[ $name = a* ]]`) or regular expressions (`[[ $(date) =~
^Fri\ ...\ 13 ]]`). In those cases, use the following pattern to replace “bare”
`[[ ... ]]`.

```
[[ ... ]] || return 1
```

References:
* [Differences between `[` and `[[`](http://mywiki.wooledge.org/BashFAQ/031)
* [Problems with `[[` in Bats](https://github.com/sstephenson/bats/issues/49)
* [Using `|| return 1` instead of `|| false`](https://github.com/bats-core/bats-core/commit/e5695a673faad4d4d33446ed5c99d70dbfa6d8be)
