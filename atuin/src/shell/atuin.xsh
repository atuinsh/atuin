$ATUIN_SESSION=$(atuin uuid).rstrip('\n')

@events.on_precommand
def _atuin_precommand(cmd: str):
    $ATUIN_HISTORY_ID=$(atuin history start -- @(cmd)).rstrip('\n')

@events.on_postcommand
def _atuin_postcommand(cmd: str, rtn: int, out, ts):
    if "ATUIN_HISTORY_ID" not in ${...}:
        return

    with ${...}.swap(ATUIN_LOG="error"):
        # This causes the entire .xonshrc to be re-executed, which is incredibly slow
        # This happens when using a subshell and using output redirection at the same time
        # For more details, see https://github.com/xonsh/xonsh/issues/5224
        # (atuin history end --exit @(rtn) -- $ATUIN_HISTORY_ID &) > /dev/null 2>&1
        atuin history end --exit @(rtn) -- $ATUIN_HISTORY_ID > /dev/null 2>&1
    del $ATUIN_HISTORY_ID

import tempfile
from prompt_toolkit.keys import Keys
@events.on_ptk_create
def _custom_keybindings(bindings, **kw):

    @bindings.add(Keys.ControlR)
    def search(event):
        # We can't use $() notation, as that would prevent the TUI from being shown
        # xonsh.lib.subprocess.check_output has the same issue, and 
        # xonsh.lib.subprocess.run can't capture output.
        # xonsh.procs.specs.SubprocSpec doesn't support redirecting output either.
        # As inefficient as it is, we have to use a temporary file
        with tempfile.NamedTemporaryFile(mode="w+", encoding="utf8") as f:
            atuin search -i -- @(event.current_buffer.text) e> @(f.name)
            cmd = f.read().rstrip('\n')
            event.current_buffer.reset()
            event.current_buffer.insert_text(cmd)

del _custom_keybindings