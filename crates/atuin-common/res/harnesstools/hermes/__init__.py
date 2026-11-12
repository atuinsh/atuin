"""Atuin's plugin for Hermes Agent: records the shell commands Hermes runs in Atuin's history,
and imports the sessions atuin writes into Hermes's store.

`atuin hook install hermes` writes this file to `$HERMES_HOME/plugins/atuin/` and runs
`hermes plugins enable atuin`.

Each `terminal` call's `pre_tool_call` and `post_tool_call` go to `atuin hook hermes` as JSON
on stdin: Hermes's own keyword arguments, plus the event's name, the directory the command runs
in, which Hermes doesn't pass, and when Hermes called the hook (`called_at`, Unix seconds), since
the event reaches atuin later. Atuin reads the rest, so this file stays small.

Hermes runs a `pre_tool_call` callback under `plugins.hook_callback_timeout` and blocks the
command when the callback overruns or raises. So the callbacks only queue the event and return
None, and one worker thread hands the events to atuin in order: a command's end never reaches
atuin before its start.

`hermes atuin import <file>` is how atuin writes a session: a restore, a fork, or a continuation
from another agent. The gateway is the only process Hermes lets write `state.db`. The only verb
that takes a whole session is `session.mutate import`, and `hermes sessions import` reaches it only
through a parser that drops tool calls. The command hands the gateway one session row, shaped as
`hermes sessions export` writes one, and prints one JSON line saying what happened: `{"ok": true,
...}` when the session was imported, else `{"ok": false, "error": ...}`. It exits 0 only when it
was.
"""

import atexit
import json
import os
import queue
import subprocess
import threading
import time
import uuid

# Replaced with atuin's absolute path at install: the gateway that runs hooks doesn't have the
# shell's PATH.
ATUIN = "atuin"

# Replaced at install with where the installing shell's atuin keeps its config and data
# (ATUIN_HOME and the like), which a gateway that launchd or systemd starts doesn't have either.
ATUIN_ENV = {}

# A hung atuin is killed after this many seconds, so it can't stall the events behind it.
_SEND_TIMEOUT_S = 10

# How long a Hermes that is exiting waits for events still queued. Before v0.22 the hooks ran
# in the CLI process, which exits when the chat ends.
_DRAIN_TIMEOUT_S = 5

_events = queue.Queue()
_worker = None
_worker_lock = threading.Lock()


def register(ctx):
    ctx.register_hook("pre_tool_call", _on_pre_tool_call)
    ctx.register_hook("post_tool_call", _on_post_tool_call)
    # Hermes before plugin commands still records commands; only atuin's writes need this.
    register_cli_command = getattr(ctx, "register_cli_command", None)
    if register_cli_command is not None:
        register_cli_command(
            "atuin", "Write the sessions atuin restores into this store", _setup_cli, _run_cli
        )


def _on_pre_tool_call(**kwargs):
    _queue_event("pre_tool_call", kwargs)
    return None


def _on_post_tool_call(**kwargs):
    _queue_event("post_tool_call", kwargs)
    return None


def _queue_event(name, kwargs):
    if kwargs.get("tool_name") != "terminal":
        return
    try:
        event = dict(kwargs)
        event["hook_event_name"] = name
        event["called_at"] = time.time()
        event["cwd"] = _command_cwd(kwargs.get("args"), kwargs.get("task_id"))
        # Encoded now, while the arguments are as Hermes passed them.
        _events.put(json.dumps(event, default=repr))
        _start_worker()
    except Exception:
        pass


def _command_cwd(args, task_id):
    """Where the command runs: its `workdir`, else the terminal's directory (which follows the
    agent's `cd`s), else the session's, else this process's (right for Hermes before v0.22).
    The lookups are Hermes internals, so each one may fail."""
    base = _terminal_cwd(task_id) or _session_cwd() or _process_cwd()
    workdir = args.get("workdir") if isinstance(args, dict) else None
    if isinstance(workdir, str) and workdir:
        return os.path.join(base, workdir) if base else workdir
    return base


def _terminal_cwd(task_id):
    try:
        from tools.approval import get_current_session_key
        from tools.terminal_tool import get_session_cwd

        return get_session_cwd(get_current_session_key(default="") or task_id) or None
    except Exception:
        return None


def _session_cwd():
    try:
        from agent.runtime_cwd import scoped_session_cwd

        return scoped_session_cwd() or None
    except Exception:
        return None


def _process_cwd():
    try:
        return os.getcwd()
    except Exception:
        return None


def _start_worker():
    global _worker
    with _worker_lock:
        if _worker is not None and _worker.is_alive():
            return
        # A daemon thread never keeps Hermes from exiting; `_drain` gives it a moment instead.
        _worker = threading.Thread(target=_send_events, name="atuin-hook", daemon=True)
        _worker.start()
        atexit.register(_drain, _worker)


def _send_events():
    while True:
        event = _events.get()
        if event is None:
            return
        _send(event)


def _send(event):
    try:
        proc = subprocess.Popen(
            [ATUIN, "hook", "hermes"],
            env={**os.environ, **ATUIN_ENV},
            stdin=subprocess.PIPE,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except Exception:
        return
    try:
        proc.communicate(event.encode("utf-8"), timeout=_SEND_TIMEOUT_S)
    except Exception:
        try:
            proc.kill()
            proc.wait()
        except Exception:
            pass


def _drain(worker):
    _events.put(None)
    worker.join(_DRAIN_TIMEOUT_S)


def _setup_cli(parser):
    actions = parser.add_subparsers(dest="atuin_action")
    imported = actions.add_parser("import", help="Import one session row (JSON) atuin wrote")
    imported.add_argument("file")


def _run_cli(args):
    if getattr(args, "atuin_action", None) != "import":
        return _reply(False, error="usage: hermes atuin import <file>", code=2)
    try:
        with open(args.file, encoding="utf-8") as f:
            row = json.load(f)
    except Exception as exc:
        return _reply(False, error=f"can't read {args.file}: {exc}")
    session_id = row.get("id") if isinstance(row, dict) else None
    if not isinstance(session_id, str) or not session_id:
        return _reply(False, error="the row names no session id")
    # The gateway's client, as `hermes sessions import` reaches it: Hermes internals, so a
    # Hermes that moved them is told apart from a refused import.
    try:
        from hermes_cli.gateway_session_store import session_store
    except Exception as exc:
        return _reply(False, error=f"unsupported_hermes: no session store to import through ({exc})")
    try:
        receipt = session_store().call(
            "session.mutate",
            session_id=session_id,
            request_id=uuid.uuid4().hex,
            # A new session's revision: an id the store holds fails `revision_conflict`.
            expected_revision=0,
            operation="import",
            payload={"sessions": [row]},
        )
    except Exception as exc:
        reason = getattr(exc, "code", None) or getattr(exc, "reason", None)
        text = str(exc) or type(exc).__name__
        if isinstance(reason, str) and reason not in text:
            text = f"{reason}: {text}"
        return _reply(False, error=text)
    imported = _count(receipt, "imported")
    if imported == 1:
        return _reply(True, receipt=receipt)
    skipped = _count(receipt, "skipped")
    return _reply(False, error="skipped" if skipped else "nothing imported", receipt=receipt)


def _count(receipt, key):
    """The count `key` of a receipt, wherever the gateway nests it: the verb's own result is
    wrapped in the receipt layer's."""
    if isinstance(receipt, dict):
        value = receipt.get(key)
        if isinstance(value, int) and not isinstance(value, bool):
            return value
        for inner in receipt.values():
            found = _count(inner, key)
            if found is not None:
                return found
    return None


def _reply(ok, code=None, **fields):
    print(json.dumps({"ok": ok, **fields}, default=repr), flush=True)
    if code is not None:
        return code
    return 0 if ok else 1
