# Desktop capture: development

How to build desktop capture from a checkout, run the daemon with a helper from
the checkout, use the library, run the test harness, and rehearse the demo in
Claude Code.

## Building

The `desktop-capture` feature of the `atuin` package builds the helper as a
second binary:

```sh
cargo build --release -p atuin --features desktop-capture
target/release/atuin setup --only --agent-history --agents claude-code
target/release/atuin setup --only --desktop-capture --daemon
target/release/atuin desktop-capture status
```

The steps are the same on Linux and on macOS. On macOS, run them in a terminal
at the Mac's screen, because setup asks for the Accessibility permission there.

A running daemon applies `atuin setup --desktop-capture` by itself, and setup
starts the daemon when the daemon isn't running. A daemon that runs with
`autostart = false` and is not running needs `atuin daemon start`.

A development build and an installed atuin can replace each other's daemon, as
[Run only this atuin](guide.md#run-only-this-atuin) explains. Keep the shell,
its hooks, the MCP server, and the daemon on one build, or give a test daemon
its own `ATUIN_HOME`, configuration, and socket, as the harness does.

## Running another helper

The daemon looks for the helper next to its own executable. The `helper_path`
setting points the daemon at another helper, such as the one in a checkout's
`target/release`:

```toml
[desktop_capture]
helper_path = "/path/to/atuin/target/release/atuin-desktop-capture"
```

A change to `helper_path` stops the running helper and starts the new one,
without a daemon restart.

## The library

```rust
use atuin_desktop_capture::{
    Backend as _, Capture, Config, Desktop, Purpose, Record,
};

let backend = Desktop::connect(Purpose::Capture).await?;
let mut capture = Capture::new(backend, Config::default());
while let Some(record) = capture.next().await {
    match record {
        // App, title, URL, start time.
        Record::SegmentStarted(segment) => {}
        // Trigger, and a changed tree or the hash of an unchanged one.
        Record::Snapshot(snapshot) => {}
        Record::SegmentEnded { segment, at } => {}
        // App, title, element tree, hash, read cost.
        Record::Dialog(dialog) => {}
        // App, title, content.
        Record::Notification(notification) => {}
        // App, title, URL, element tree, hash, read cost.
        Record::Background(background) => {}
    }
}
```

The library's modules and how they fit together are described in [the
library](architecture/library.md).

## The test harness

[The harness](../harness/README.md) runs capture against real applications in
private X11, Wayland, and macOS sessions. Each scenario runs capture through a
private Atuin daemon, so a run leaves the account's own daemon and data alone.

## Rehearsing the demo in Claude Code

The demo asks Claude Code about a page that the user just read, without naming
the page. These steps use the real Atuin config and the real
`~/.claude/settings.json`. Run them from the Atuin checkout.

1. Build Atuin and the helper, and name the build:

   ```sh
   cargo build --release -p atuin --features desktop-capture
   A=$PWD/target/release/atuin
   ```

2. Add this section to Atuin's config file, with the checkout's path in
   `helper_path`:

   ```toml
   [desktop_capture]
   apps = ["Firefox", "zenity"]
   helper_path = "/path/to/atuin/target/release/atuin-desktop-capture"
   ```

3. Install Atuin's Claude Code hooks and its MCP server, which gives Claude
   Code the `atuin_desktop_*` tools. The hook command in the settings uses the
   absolute path of `$A`.

   ```sh
   $A setup --only --agent-history --mcp --agents claude-code
   ```

4. Turn capture on, and list the applications. Run setup in a terminal, because
   on macOS, setup creates the certificate and asks for the Accessibility
   permission on its last page. Setup also registers the `UserPromptSubmit`
   hook.

   ```sh
   $A setup --desktop-capture --daemon --desktop-apps listed
   $A desktop-capture apps
   ```

   A running daemon applies the change by itself. A daemon that runs with
   `autostart = false` and is not running needs `$A daemon start` in a terminal
   of its own.

5. Check the setup:

   ```sh
   $A desktop-capture status
   jq .hooks.UserPromptSubmit ~/.claude/settings.json
   ```

   The status must say `Desktop capture: running`. The `jq` output must list
   `atuin hook claude-code`.

6. Open https://www.open-std.org/jtc1/sc22/wg14/www/docs/n3254.pdf in Firefox,
   and keep it in front for 10 seconds. `$A desktop-capture log --limit 3` must
   list a Firefox segment whose title or URL contains n3254.

7. Start a new Claude Code session, and ask "does my compiler have any issues
   related to n3254?".

8. In the same session, ask Claude Code which desktop activity the prompt came
   with. The answer must quote a line that begins with a time and names the
   page or contains n3254.

The second demo asks about an error that vanished before the user could read
it. These steps are for Linux, where zenity shows the error. They follow
step 8.

9. Show an error dialog for 2 seconds:

   ```sh
   zenity --error --text "Copy failed: no space left on device" &
   sleep 2; kill $!
   ```

   `$A desktop-capture log --limit 3` must list a zenity segment of the kind
   `dialog`.

10. In the same Claude Code session, ask "what was that error?". The prompt's
    context holds a line `<time> dialog in zenity: Error`. To answer, Claude
    Code reads the dialog with `atuin_desktop_read`, so the answer must quote
    "Copy failed: no space left on device".

To watch the hook run, start Claude Code with
`claude --debug=hooks --debug-file /tmp/claude-hooks.log`, and look for the
`UserPromptSubmit` hook in that file.

To undo the setup, run `$A setup --only --no-desktop-capture`, which also takes
the prompt hook out of `~/.claude/settings.json`. Afterwards,
`jq .hooks.UserPromptSubmit ~/.claude/settings.json` must no longer list
`atuin hook claude-code`.

To see the context without Claude Code, pipe a recorded event into the hook:

```sh
echo '{"session_id": "rehearsal", "transcript_path": "/dev/null",
  "cwd": "'"$PWD"'", "hook_event_name": "UserPromptSubmit",
  "prompt": "does my compiler have any issues related to n3254?"}' |
  $A hook claude-code
```

The hook prints a JSON object. Its `hookSpecificOutput.additionalContext` holds
the lines. The hook keeps what it showed to each session in
`$TMPDIR/atuin-prompt-context-<session id>`. A second run within the same
session prints only what is new. The harness's `daemon` scenario runs the hook
this way in private sessions, with a local stand-in for the page.
