# Atuin's OpenCode plugins

The V1 and V2 loaders have incompatible export contracts. Install the variant
for the API your OpenCode build uses; do not install both templates together.

```sh
# V1, including older loaders that invoke every export as a function:
atuin hook install opencode

# V2 builds exposing tool.hook and shell.hook:
atuin hook install opencode-v2
```

Both commands write `${XDG_CONFIG_HOME:-$HOME/.config}/opencode/plugins/atuin.ts`,
replacing the other variant. Restart OpenCode after switching. V1 exports only
a function; V2 exports only an `{ id, setup }` object. Both templates are
standalone so the installed file needs no sibling modules. The V2 installer
option requires an Atuin build containing this change; it is not an instruction
for already-released Atuin binaries.

The V2 API is still changing. This adapter feature-detects the `tool.hook` and
`shell.hook` domains and quietly disables itself on builds without them.
The test fixtures below exercise that hook contract, not a live OpenCode beta.

## Correlation and failure policy

V1 associates the post-permission shell event with the exact tool call ID.
The V2 shell event used by this adapter does not supply a tool call ID. Its
command-text fallback therefore records only non-overlapping, unambiguous
proposals. When identical commands overlap, their proposals remain ineligible
until they finish, even if one is denied or finishes first. An already-started
entry is still completed using its original tool ID. This deliberately loses
some history rather than swapping intents, working directories, or exit codes.
It does not claim to distinguish an unrelated user shell that executes the
same command as a sole pending tool proposal; that needs a host-provided call ID.

At the pending-proposal limit, V2 stops correlating new commands until reload
instead of evicting a proposal that could later steal another call's entry.
Already-started entries can still finish. Registration failures roll back
previous registrations; callbacks stay inert until setup completes and after
disposal, including when the host's disposer fails. As with the original
adapter, a host shutdown/error that omits completion can leave history open.

## Regression tests

From the repository root, with Node.js 22.6 or newer:

```sh
node --experimental-strip-types --test crates/atuin/contrib/opencode/tests/atuin.test.mjs
```

No npm install is needed. The suite uses Node's built-in test runner and mocks
only the Atuin subprocess. It checks loader exports, reordered/overlapping
calls, denied commands, cwd fallback, setup rollback, disposal, bounded state,
argument preservation, subprocess failures, and completion exit codes.
The Rust installer cases live alongside `hook.rs` and run with the crate tests.
