import os
import subprocess

from prompt_toolkit.application.current import get_app
from prompt_toolkit.filters import Condition
from prompt_toolkit.keys import Keys


if "ATUIN_SESSION" not in ${...} or ${...}.get("ATUIN_SHLVL", "") != ${...}.get("SHLVL", ""):
    $ATUIN_SESSION=$(atuin uuid).rstrip('\n')
    $ATUIN_SHLVL = ${...}.get("SHLVL", "")

@events.on_precommand
def _atuin_precommand(cmd: str):
    cmd = cmd.rstrip("\n")
    try:
        $ATUIN_HISTORY_ID = $($ATUIN_SHELL="xonsh" atuin history start --hook -- @(cmd) 2>@(os.devnull)).rstrip("\n")
    except:
        $ATUIN_HISTORY_ID = ""


@events.on_postcommand
def _atuin_postcommand(cmd: str, rtn: int, out, ts):
    if "ATUIN_HISTORY_ID" not in ${...}:
        return

    duration = ts[1] - ts[0]
    nanos = max(0, round(duration * 10 ** 9))

    args = ["history", "end", "--hook", "--exit", str(rtn), "--duration", str(nanos), "--", $ATUIN_HISTORY_ID]

    # Run in the background, so a slow daemon can't hold up the prompt. Not as a `&` job: using a
    # subshell and output redirection together re-executes the entire .xonshrc, which is incredibly
    # slow. For more details, see https://github.com/xonsh/xonsh/issues/5224
    #
    # An `atuin` alias comes back expanded into its command line, unless it's a function alias.
    # Only xonsh can run those, so they still run in the foreground: a thread would be killed with
    # the shell, losing a command the daemon was still waiting to hear the end of.
    atuin_cmd = aliases.get("atuin", ["atuin"])
    started = False
    if all(isinstance(arg, str) for arg in atuin_cmd):
        try:
            subprocess.Popen(
                [*atuin_cmd, *args],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                env=${...}.detype(),
                start_new_session=True,
            )
            started = True
        except OSError:
            pass

    if not started:
        atuin @(args) > @(os.devnull) 2>&1
    del $ATUIN_HISTORY_ID


def _search(event, extra_args: list[str]):
    buffer = event.current_buffer
    cmd = ["atuin", "search", "--interactive", *extra_args]
    # We need to explicitly pass in xonsh env, in case user has set XDG_HOME or something else that matters
    env = ${...}.detype()
    env["ATUIN_SHELL"] = "xonsh"
    env["ATUIN_QUERY"] = buffer.text

    p = subprocess.run(cmd, stderr=subprocess.PIPE, encoding="utf-8", env=env)
    result = p.stderr.rstrip("\n")
    # redraw prompt - necessary if atuin is configured to run inline, rather than fullscreen
    event.cli.renderer.erase()

    if not result:
        return

    buffer.reset()
    if result.startswith("__atuin_accept__:"):
        buffer.insert_text(result[17:])
        buffer.validate_and_handle()
    else:
        buffer.insert_text(result)


@events.on_ptk_create
def _custom_keybindings(bindings, **kw):
    if _ATUIN_BIND_CTRL_R:
        @bindings.add(Keys.ControlR)
        def r_search(event):
            _search(event, extra_args=[])

    if _ATUIN_BIND_UP_ARROW:
        @Condition
        def should_search():
            buffer = get_app().current_buffer
            # disable keybind when there is an active completion, so
            # that up arrow can be used to navigate completion menu
            if buffer.complete_state is not None:
                return False
            # similarly, disable when buffer text contains multiple lines
            if '\n' in buffer.text:
                return False

            return True

        @bindings.add(Keys.Up, filter=should_search)
        def up_search(event):
            _search(event, extra_args=["--shell-up-key-binding"])


def _atuin_prepare_search_index():
    env = ${...}.detype()
    env["ATUIN_SHELL"] = "xonsh"
    try:
        # Launch process in the background to avoid blocking the shell.
        subprocess.Popen(
            ["atuin", "__internal", "prepare-search-index"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            env=env,
            start_new_session=True,
        )
    except OSError:
        # Ignore errors; `prepare-search-index` is an optimization only.
        pass


_atuin_prepare_search_index()
del _atuin_prepare_search_index
