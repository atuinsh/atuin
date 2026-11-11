"""Atuin's plugin for Hermes Agent: records the shell commands Hermes runs in Atuin's history.

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
"""

import atexit
import json
import os
import queue
import subprocess
import threading
import time

# Replaced with atuin's absolute path at install: the gateway that runs hooks doesn't have the
# shell's PATH.
ATUIN = "atuin"

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
