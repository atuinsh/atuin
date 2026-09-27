#!/usr/bin/env python3
"""Splits the test suite across a step's parallel jobs by recorded timings.

The build-tests steps compile the tests once into a nextest archive and plan
how to split them; each parallel test job then runs its share of the
archive. nextest's own `--partition` splits by count, so one slow test makes
one job slow. This balances by how long each test took on main instead.

  plan    <list.json> <timings.json> <os> <jobs> <plan.json>
            From `cargo nextest list --message-format json` and the recorded
            timings (which may be missing), assign every test to one of
            <jobs> jobs, longest first, each to the job with the least work
            so far. Tests with no timing yet count as the median.
  run     <plan.json> -- <nextest run command...>
            Run the command with this job's filters (-E, one per test
            binary) appended. The job is BUILDKITE_PARALLEL_JOB.
  record  <old-timings.json> <new-timings.json> <junit.xml...>
            Merge JUnit reports into the timings file. An OS with reports in
            this build is replaced wholesale (so deleted tests drop out);
            other OSes keep their previous timings.

Report names carry their OS (junit-<os>-<job id>.xml, from the
post-command hook). Timings are keyed by "<binary id> <test name>", which is
how both `nextest list` and nextest's JUnit report name a test.
"""

import json
import os
import re
import statistics
import sys
import xml.etree.ElementTree as ET

# Names that can go in a nextest filter as-is: Rust test paths, and binary
# IDs like `atuin-client::scale` or `atuin::bin/atuin`. Anything else keeps
# its whole binary on one job rather than risk a filter that silently
# matches the wrong tests.
PLAIN_NAME = re.compile(r"^[A-Za-z0-9_:]+$")
PLAIN_BINARY_ID = re.compile(r"^[A-Za-z0-9_:/-]+$")


def load_json(path, default):
    try:
        with open(path) as f:
            return json.load(f)
    except FileNotFoundError:
        return default


def plan(list_path, timings_path, os_name, jobs, out_path):
    jobs = int(jobs)
    listing = load_json(list_path, None)
    timings = load_json(timings_path, {}).get(os_name, {})
    default = statistics.median(timings.values()) if timings else 1.0

    # Units of work: single tests, or whole binaries whose test names can't
    # be filtered on individually.
    units = []  # (seconds, binary id, [test names])
    for binary_id, suite in sorted(listing["rust-suites"].items()):
        names = sorted(
            name
            for name, case in suite["testcases"].items()
            if not case["ignored"] and case["filter-match"]["status"] == "matches"
        )
        if not names:
            continue
        seconds = [timings.get(f"{binary_id} {name}", default) for name in names]
        if PLAIN_BINARY_ID.match(binary_id) and all(PLAIN_NAME.match(n) for n in names):
            units += [(s, binary_id, [n]) for s, n in zip(seconds, names)]
        else:
            units.append((sum(seconds), binary_id, names))
    if not units:
        sys.exit("test-plan: no tests to run; is the archive empty?")

    loads = [0.0] * jobs
    assigned = [{} for _ in range(jobs)]  # job -> binary id -> [names]
    for seconds, binary_id, names in sorted(units, key=lambda u: (-u[0], u[1], u[2])):
        job = loads.index(min(loads))
        loads[job] += seconds
        assigned[job].setdefault(binary_id, []).extend(names)

    all_names = {}
    for _, binary_id, names in units:
        all_names.setdefault(binary_id, set()).update(names)

    partitions = []
    for job in range(jobs):
        filters = []
        for binary_id, names in sorted(assigned[job].items()):
            names = set(names)
            others = all_names[binary_id] - names
            if not others:
                expr = f"binary_id(={binary_id})"
            elif len(names) <= len(others):
                expr = f"binary_id(={binary_id}) & ({' | '.join(f'test(={n})' for n in sorted(names))})"
            else:
                expr = f"binary_id(={binary_id}) & not ({' | '.join(f'test(={n})' for n in sorted(others))})"
            filters.append(expr)
        partitions.append(filters)

    with open(out_path, "w") as f:
        # The test jobs need the build's checkout path: the test binaries
        # have it compiled in (run-tests.sh).
        json.dump(
            {"os": os_name, "jobs": jobs, "checkout": os.getcwd(), "partitions": partitions},
            f,
        )

    tests = sum(len(n) for _, _, n in units)
    known = sum(1 for _, b, ns in units for n in ns if f"{b} {n}" in timings)
    print(f"Planned {tests} tests ({known} with recorded timings) across {jobs} jobs:")
    for job, load in enumerate(loads):
        count = sum(len(n) for n in assigned[job].values())
        print(f"  job {job}: {count:5} tests, {load:7.1f}s of test time")


def run(plan_path, command):
    with open(plan_path) as f:
        planned = json.load(f)
    job = int(os.environ.get("BUILDKITE_PARALLEL_JOB", "0"))
    count = int(os.environ.get("BUILDKITE_PARALLEL_JOB_COUNT", "1"))
    if count != planned["jobs"]:
        sys.exit(
            f"test-plan: planned for {planned['jobs']} jobs but this step has "
            f"parallelism {count}; keep TEST_JOBS and parallelism in sync"
        )
    args = command[:]
    for expr in planned["partitions"][job]:
        args += ["-E", expr]
    # A job can get no tests when there are fewer tests than jobs.
    args.append("--no-tests=pass")
    os.execvp(args[0], args)


def record(old_path, new_path, reports):
    timings = load_json(old_path, {})
    fresh = {}
    for report in reports:
        if not os.path.exists(report):  # an unmatched shell glob
            continue
        match = re.search(r"junit-([a-z]+)-", os.path.basename(report))
        if not match:
            continue
        per_os = fresh.setdefault(match.group(1), {})
        for case in ET.parse(report).getroot().iter("testcase"):
            key = f"{case.get('classname')} {case.get('name')}"
            # A retried test reports every attempt; keep the slowest.
            per_os[key] = max(per_os.get(key, 0.0), float(case.get("time") or 0))
    timings.update(fresh)
    with open(new_path, "w") as f:
        json.dump(timings, f, sort_keys=True, indent=0)
    for os_name, per_os in sorted(timings.items()):
        state = "updated" if os_name in fresh else "kept"
        print(f"{os_name}: {len(per_os)} tests ({state})")


if __name__ == "__main__":
    mode, rest = sys.argv[1], sys.argv[2:]
    if mode == "plan":
        plan(*rest)
    elif mode == "run":
        split = rest.index("--")
        run(rest[0], rest[split + 1 :])
    elif mode == "record":
        record(rest[0], rest[1], rest[2:])
    else:
        sys.exit(f"test-plan: unknown mode {mode}")
