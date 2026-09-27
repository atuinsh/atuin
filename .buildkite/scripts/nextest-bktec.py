#!/usr/bin/env python3
"""Glue between Buildkite Test Engine's client (bktec) and cargo-nextest.

bktec asks Test Engine how to split the tests across a step's parallel jobs,
by how long each took before, then runs this job's share. It has no nextest
support, so it uses its custom runner: tests are "selectors", one per test,
written `<binary id>@<test name>`.

  selectors <list.json> <selectors.txt>
      Write the selectors for every test `cargo nextest list
      --message-format json` would run.
  run <selector>...
      bktec's test command. Runs $NEXTEST_COMMAND on just these tests, then
      converts nextest's JUnit report into Test Engine JSON at
      $BUILDKITE_TEST_ENGINE_RESULT_PATH, which bktec reads and (when
      configured) uploads. Each result is tagged with its selector
      (test.selector.primary), which is how Test Engine matches timings to
      the selectors of later plans. Exits with nextest's status.
"""

import json
import os
import shlex
import subprocess
import sys
import uuid
import xml.etree.ElementTree as ET

# .config/nextest.toml's ci profile (NEXTEST_PROFILE=ci) writes it here.
JUNIT = "test-results/junit.xml"
# Keeps each -E well under Linux's 128 KiB limit on a single argument.
TESTS_PER_FILTER = 500


def selectors(list_path, out_path):
    with open(list_path) as f:
        listing = json.load(f)
    lines = [
        f"{binary_id}@{name}"
        for binary_id, suite in sorted(listing["rust-suites"].items())
        for name, case in sorted(suite["testcases"].items())
        if case["filter-match"]["status"] == "matches"
    ]
    if not lines:
        sys.exit("nextest-bktec: no tests to run")
    with open(out_path, "w") as f:
        f.write("\n".join(lines) + "\n")
    print(f"{len(lines)} tests")


def run(chosen):
    by_binary = {}
    for selector in chosen:
        binary_id, _, name = selector.partition("@")
        by_binary.setdefault(binary_id, []).append(name)

    # nextest runs the union of its -E filters.
    args = shlex.split(os.environ["NEXTEST_COMMAND"])
    for binary_id, names in sorted(by_binary.items()):
        for i in range(0, len(names), TESTS_PER_FILTER):
            tests = " | ".join(f"test(={n})" for n in names[i : i + TESTS_PER_FILTER])
            args += ["-E", f"binary_id(={binary_id}) & ({tests})"]

    if os.path.exists(JUNIT):
        os.remove(JUNIT)
    status = subprocess.call(args)
    write_results(set(chosen))
    sys.exit(status)


def write_results(chosen):
    results = []
    if os.path.exists(JUNIT):
        for case in ET.parse(JUNIT).getroot().iter("testcase"):
            binary_id, name = case.get("classname"), case.get("name")
            selector = f"{binary_id}@{name}"
            failure = case.find("failure")
            if failure is None:
                failure = case.find("error")
            if failure is not None:
                result = "failed"
            elif case.find("skipped") is not None:
                result = "skipped"
            else:
                result = "passed"
            entry = {
                "id": str(uuid.uuid4()),
                "scope": binary_id,
                "name": name,
                "result": result,
                "history": {"start_at": 0, "duration": float(case.get("time") or 0)},
                "tags": {"test.selector.primary": selector},
            }
            if failure is not None:
                entry["failure_reason"] = (failure.get("message") or failure.get("type") or "")[:1000]
            if selector not in chosen:
                print(f"nextest-bktec: ran {selector}, which wasn't assigned to this job")
            results.append(entry)
    path = os.environ.get("BUILDKITE_TEST_ENGINE_RESULT_PATH")
    if path:
        os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
        with open(path, "w") as f:
            json.dump(results, f)


if __name__ == "__main__":
    mode, rest = sys.argv[1], sys.argv[2:]
    if mode == "selectors":
        selectors(*rest)
    elif mode == "run":
        run(rest)
    else:
        sys.exit(f"nextest-bktec: unknown mode {mode}")
