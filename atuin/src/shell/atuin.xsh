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