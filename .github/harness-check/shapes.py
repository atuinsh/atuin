"""Print the distinct record shapes a harness wrote, one per line, sorted.

Usage: shapes.py <path>...  (JSONL files are found recursively; *.db files are read as SQLite)

A JSONL shape is the line's `type` plus its discriminating sub-type (payload.type, subtype,
message.role or attachment.type) and the types of its content blocks. A SQLite shape is each
non-empty table plus the distinct `type` column values and `data` JSON type/role values.
The parsers skip what they do not know, so a shape missing from the checked-in list is the
signal that a release writes something they have never seen."""
import json
import pathlib
import sqlite3
import sys


def jsonl_shapes(path):
    for line in path.open(encoding="utf-8", errors="replace"):
        try:
            d = json.loads(line)
        except ValueError:
            yield "unparseable line"
            continue
        if not isinstance(d, dict):
            continue
        inner = next((d[k] for k in ("payload", "message", "attachment") if isinstance(d.get(k), dict)), {})
        sub = inner.get("type") or d.get("subtype") or inner.get("role")
        base = f"{d.get('type')}/{sub}" if sub else str(d.get("type"))
        yield base
        content = inner.get("content")
        for block in content if isinstance(content, list) else []:
            if isinstance(block, dict):
                yield f"{base} > {block.get('type')}"


def sqlite_shapes(path):
    db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    tables = [r[0] for r in db.execute("select name from sqlite_master where type='table'")]
    for t in tables:
        if t.startswith(("sqlite_", "__")) or not db.execute(f'select 1 from "{t}" limit 1').fetchone():
            continue
        yield f"table {t}"
        cols = [r[1] for r in db.execute(f'pragma table_info("{t}")')]
        if "type" in cols:
            for (v,) in db.execute(f'select distinct type from "{t}"'):
                yield f"{t}.type {v}"
        if "data" in cols:
            for (v,) in db.execute(f'select distinct json_extract(data, \'$.type\') from "{t}" where json_valid(data)'):
                if v is not None:
                    yield f"{t}.data.type {v}"
            for (v,) in db.execute(f'select distinct json_extract(data, \'$.role\') from "{t}" where json_valid(data)'):
                if v is not None:
                    yield f"{t}.data.role {v}"


shapes = set()
for arg in sys.argv[1:]:
    root = pathlib.Path(arg)
    for p in [root] if root.is_file() else sorted(root.rglob("*")):
        if p.suffix == ".jsonl":
            shapes.update(jsonl_shapes(p))
        elif p.suffix == ".db":
            shapes.update(sqlite_shapes(p))
print("\n".join(sorted(shapes)))
