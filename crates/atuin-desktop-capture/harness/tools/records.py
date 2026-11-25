"""Prints what Atuin's daemon stored in a scenario, with the scenario's marks, in time order.

Usage: records.py <work directory> [app]

The work directory holds what capture_stop saved: segments.ndjson and snapshots.ndjson, in the
JSON styles of `atuin desktop-capture log` and `snapshots`. Each line of `marks` holds the
milliseconds since the Unix epoch, a space, and the text of a mark.

The lines, where <word> is "segment" for an active segment and the kind of any other segment:
    <word> <id>: <app> — <title>                    a segment starts
    <word> <id>: <trigger> snapshot, <n> nodes      a snapshot, with ", unchanged" when its
                                                    content is the same as the one before
    == <text>                                       a mark
A mark comes before the segments that started in the same millisecond.

With [app], a regular expression, only the segments of the applications whose names match are
printed, with their snapshots."""
import json
import os
import re
import sys

work = sys.argv[1]
app = re.compile(sys.argv[2]) if len(sys.argv) > 2 else None


def wanted(name):
    return app is None or app.search(name) is not None


def objects(name):
    path = os.path.join(work, name)
    if not os.path.exists(path):
        return []
    with open(path) as lines:
        return [json.loads(line) for line in lines if line.strip()]


entries = []
words = {}
for segment in objects("segments.ndjson"):
    if not wanted(segment["app"]):
        continue
    word = "segment" if segment["kind"] == "active" else segment["kind"]
    words[segment["segment"]] = word
    entries.append((segment["started_unix_ms"], 1,
                    f"{word} {segment['segment']}: {segment['app']} — {segment['title']}"))
for snapshot in objects("snapshots.ndjson"):
    if snapshot["segment"] not in words:
        continue
    word = words[snapshot["segment"]]
    trigger = f"{snapshot['trigger']} " if snapshot["trigger"] else ""
    unchanged = ", unchanged" if snapshot["same_as_previous"] else ""
    entries.append((snapshot["at_unix_ms"], 2,
                    f"{word} {snapshot['segment']}: {trigger}snapshot, {snapshot['nodes']} nodes{unchanged}"))
marks = os.path.join(work, "marks")
if os.path.exists(marks):
    with open(marks) as lines:
        for line in lines:
            at, text = line.rstrip("\n").split(" ", 1)
            entries.append((int(at), 0, f"== {text}"))
for _, _, text in sorted(entries, key=lambda entry: entry[:2]):
    print(text)
