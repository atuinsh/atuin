"""Tiny scripted model server for harness-check: Anthropic Messages, OpenAI Responses and
OpenAI Chat Completions, all streamed. A prompt containing SHELL gets one shell tool call;
everything else gets a short text reply. Run it where only loopback is reachable."""
import itertools
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CMD = "echo harness-check"
SHELLS = ("Bash", "bash", "exec_command", "shell")
ids = itertools.count(1)


def text_of(content):
    if isinstance(content, str):
        return content
    return " ".join(b.get("text", "") for b in content or [] if isinstance(b, dict))


def shell_call(tools):
    """(tool name, JSON arguments) for the request's shell tool, if it offers one."""
    names = [t.get("name") or t.get("function", {}).get("name") for t in tools or []]
    name = next((n for n in SHELLS if n in names), None)
    if name == "exec_command":
        return name, json.dumps({"cmd": CMD})
    return name, json.dumps({"command": CMD, "description": "echo"})


def plan(req, messages, after_tool):
    """None for a text reply, else the (name, args) tool call to make."""
    users = [m for m in messages if m.get("role") == "user"]
    prompt = text_of(users[-1].get("content")) if users else ""
    if after_tool or "SHELL" not in prompt:
        return None
    name, args = shell_call(req.get("tools"))
    return (name, args) if name else None


def sse(events, named):
    out = ""
    for e in events:
        out += (f"event: {e['type']}\n" if named else "") + f"data: {json.dumps(e)}\n\n"
    return out + ("" if named else "data: [DONE]\n\n")


def anthropic(req, n):
    msgs = req.get("messages", [])
    last = msgs[-1].get("content") if msgs else ""
    after = isinstance(last, list) and any(b.get("type") == "tool_result" for b in last)
    call = plan(req, msgs, after)
    block = ({"type": "tool_use", "id": f"toolu_{n}", "name": call[0], "input": {}} if call
             else {"type": "text", "text": ""})
    delta = ({"type": "input_json_delta", "partial_json": call[1]} if call
             else {"type": "text_delta", "text": f"mock reply {n}"})
    usage = {"input_tokens": 100, "output_tokens": 10, "cache_read_input_tokens": 0,
             "cache_creation_input_tokens": 0}
    return sse([
        {"type": "message_start", "message": {"id": f"msg_{n}", "type": "message", "role": "assistant",
                                              "model": req.get("model"), "content": [], "stop_reason": None,
                                              "usage": usage}},
        {"type": "content_block_start", "index": 0, "content_block": block},
        {"type": "content_block_delta", "index": 0, "delta": delta},
        {"type": "content_block_stop", "index": 0},
        {"type": "message_delta", "delta": {"stop_reason": "tool_use" if call else "end_turn"},
         "usage": {"output_tokens": 10}},
        {"type": "message_stop"},
    ], True)


def responses(req, n):
    items = req.get("input", [])
    after = bool(items) and str(items[-1].get("type", "")).endswith("_output")
    msgs = [i for i in items if i.get("type") == "message"]
    call = plan(req, msgs, after)
    item = ({"type": "function_call", "id": f"fc_{n}", "call_id": f"call_{n}", "name": call[0],
             "arguments": call[1]} if call
            else {"type": "message", "role": "assistant", "id": f"msg_{n}",
                  "content": [{"type": "output_text", "text": f"mock reply {n}"}]})
    usage = {"input_tokens": 100, "input_tokens_details": {"cached_tokens": 0}, "output_tokens": 10,
             "output_tokens_details": {"reasoning_tokens": 0}, "total_tokens": 110}
    return sse([
        {"type": "response.created", "response": {"id": f"resp_{n}"}},
        {"type": "response.output_item.done", "item": item},
        {"type": "response.completed", "response": {"id": f"resp_{n}", "usage": usage}},
    ], True)


def chat(req, n):
    msgs = req.get("messages", [])
    call = plan(req, msgs, bool(msgs) and msgs[-1].get("role") == "tool")
    delta = ({"role": "assistant", "tool_calls": [{"index": 0, "id": f"call_{n}", "type": "function",
                                                   "function": {"name": call[0], "arguments": call[1]}}]}
             if call else {"role": "assistant", "content": f"mock reply {n}"})
    base = {"id": f"chatcmpl-{n}", "object": "chat.completion.chunk", "created": 0, "model": req.get("model")}
    usage = {"prompt_tokens": 100, "completion_tokens": 10, "total_tokens": 110}
    return sse([
        dict(base, choices=[{"index": 0, "delta": delta, "finish_reason": None}]),
        dict(base, choices=[{"index": 0, "delta": {}, "finish_reason": "tool_calls" if call else "stop"}]),
        dict(base, choices=[], usage=usage),
    ], False)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def reply(self, ctype, body):
        body = body.encode()
        self.send_response(200)
        self.send_header("content-type", ctype)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        self.reply("application/json", json.dumps({"data": [], "models": []}))

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        n = next(ids)
        if self.path.endswith("/messages/count_tokens"):
            return self.reply("application/json", json.dumps({"input_tokens": 100}))
        for suffix, handler in (("/messages", anthropic), ("/responses", responses), ("/chat/completions", chat)):
            if self.path.split("?")[0].endswith(suffix):
                return self.reply("text/event-stream", handler(req, n))
        self.send_error(404)


ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
