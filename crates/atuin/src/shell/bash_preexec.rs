//! Provides a patched version of bash-preexec.
//!
//! Certain versions of VTE break bash-preexec on Bash >= 5.3 due to an issue with PS0 handling.
//! This module provides a patched version of bash-preexec that uses the old, non-PS0 mechanism with
//! these broken VTE versions. See <https://github.com/atuinsh/atuin/issues/3456>.

const VENDORED: &str = include_trimmed!("../../vendor/bash-preexec/bash-preexec.sh");

/// The part of bash-preexec that picks the PS0 hook on Bash >= 5.3.
const ANCHOR: &str = r"
if (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 3) )); then
    __bp_hook_preexec_proc=__bp_hook_preexec_into_ps0
";

/// Spliced in right after [`ANCHOR`].
///
/// The `TERM` patterns are the ones `vte.sh` checks before doing anything, unchanged across every
/// VTE release from 0.77.0 through 0.84.1.
const PATCH: &str = r"
    # atuin patch: use the DEBUG trap instead on VTE versions that break bash-preexec's PS0 hook
    # (https://github.com/atuinsh/atuin/issues/3456). Only do this for `$TERM`s that vte.sh runs on.
    if [[ ${TERM-} == xterm* || ${TERM-} == vte* || ${TERM-} == gnome* ]] &&
       [[ ${VTE_VERSION-} =~ ^[1-9][0-9]*$ ]] &&
       (( VTE_VERSION >= 7790 && VTE_VERSION < 8401 &&
          !(VTE_VERSION >= 8204 && VTE_VERSION < 8300) ))
    then
        __bp_hook_preexec_proc=__bp_hook_preexec_into_debug
    fi
";

/// The script before and after [`ANCHOR`].
const PARTS: [&str; 2] = const_format::str_split!(VENDORED, ANCHOR);

/// The patched version of bash-preexec.
pub const PATCHED: &str = const_format::concatcp!(PARTS[0], ANCHOR, PATCH, PARTS[1]);

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::{ANCHOR, PATCH, PATCHED};

    #[rstest]
    fn patch_follows_anchor() {
        assert_eq!(PATCHED.matches(&format!("{ANCHOR}{PATCH}")).count(), 1);
    }

    /// Runs [`PATCH`] as it would run in bash-preexec's bash >= 5.3 branch. The test doesn't
    /// include the [`ANCHOR`] itself, since it opens that branch and the host bash may be < 5.3.
    #[cfg(unix)]
    #[rstest]
    #[case::unset(None, false)]
    #[case::empty(Some(""), false)]
    #[case::garbage(Some("a[$(echo pwned >&2)]"), false)]
    #[case::before_termprops(Some("7600"), false)]
    #[case::first_broken(Some("7790"), true)]
    #[case::broken_0_80(Some("8005"), true)]
    #[case::last_broken_0_82(Some("8203"), true)]
    #[case::fixed_0_82(Some("8204"), false)]
    #[case::dev_0_83(Some("8391"), true)]
    #[case::broken_0_84(Some("8400"), true)]
    #[case::fixed_0_84(Some("8401"), false)]
    #[case::newer(Some("8600"), false)]
    fn old_vte_uses_debug_trap(
        #[case] vte_version: Option<&str>,
        #[case] old_vte: bool,
        #[values(
            (Some("xterm-256color"), true),
            (Some("vte-256color"), true),
            (Some("gnome"), true),
            (Some("tmux-256color"), false),
            (Some("screen"), false),
            (Some(""), false),
            (None, false)
        )]
        term: (Option<&str>, bool),
    ) {
        let (term, vte_term) = term;
        let script = format!(
            "__bp_hook_preexec_proc=__bp_hook_preexec_into_ps0\n{PATCH}echo \
             \"$__bp_hook_preexec_proc\""
        );
        let mut cmd = std::process::Command::new("bash");
        cmd.args(["--norc", "--noprofile", "-c", &script]);
        for (name, value) in [("VTE_VERSION", vte_version), ("TERM", term)] {
            match value {
                Some(v) => cmd.env(name, v),
                None => cmd.env_remove(name),
            };
        }
        let out = cmd.output().expect("failed to run bash");
        assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
        let expected = if old_vte && vte_term {
            "debug"
        } else {
            "ps0"
        };
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("__bp_hook_preexec_into_{expected}\n")
        );
    }
}
