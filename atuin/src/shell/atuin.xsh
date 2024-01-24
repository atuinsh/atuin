import subprocess
from prompt_toolkit.keys import Keys

$ATUIN_SESSION=$(atuin uuid).rstrip('\n')

@events.on_precommand
def _atuin_precommand(cmd: str):
    cmd = cmd.rstrip('\n')
    $ATUIN_HISTORY_ID = $(atuin history start -- @(cmd)).rstrip('\n')

@events.on_postcommand
def _atuin_postcommand(cmd: str, rtn: int, out, ts):
    if "ATUIN_HISTORY_ID" not in ${...}:
        return

    duration = ts[1] - ts[0]
    # Duration is float representing seconds, but atuin expects integer of nanoseconds
    nanos = round(duration * 10 ** 9)
    with ${...}.swap(ATUIN_LOG="error"):
        # This causes the entire .xonshrc to be re-executed, which is incredibly slow
        # This happens when using a subshell and using output redirection at the same time
        # For more details, see https://github.com/xonsh/xonsh/issues/5224
        # (atuin history end --exit @(rtn) -- $ATUIN_HISTORY_ID &) > /dev/null 2>&1
        atuin history end --exit @(rtn) --duration @(nanos) -- $ATUIN_HISTORY_ID > /dev/null 2>&1
    del $ATUIN_HISTORY_ID

@events.on_ptk_create
def _custom_keybindings(bindings, **kw):

    @bindings.add(Keys.ControlR, filter=_ATUIN_BIND_CTRL_R)
    def r_search(event):
        buffer = event.current_buffer
        cmd = ['atuin', 'search', '--interactive', '--', buffer.text]
        # We need to explicitly pass in xonsh env, in case user has set XDG_HOME or something else that matters
        env = ${...}.detype()
        env['ATUIN_SHELL_XONSH'] = 't'

        p = subprocess.run(cmd, stderr=subprocess.PIPE, encoding='utf-8', env=env)
        result = p.stderr.rstrip('\n')
        # redraw prompt - necessary if atuin is configured to run inline, rather than fullscreen
        event.cli.renderer.erase()

        if result == '':
            return

        buffer.reset()
        if result[:17] == '__atuin_accept__:':
            buffer.insert_text(result[17:])
            buffer.validate_and_handle()
        else:
            buffer.insert_text(result)
