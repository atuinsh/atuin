#!/usr/bin/env bun
/**
 * Analyze span timing JSON logs generated with ATUIN_SPAN
 *
 * Usage: bun scripts/span-table.ts <file.json> [options]
 *   --filter <pattern>  Only show spans matching pattern (regex)
 *   --sort <field>      Sort by: calls, avg, total, p99 (default: total)
 *   --top <n>           Show top N spans (default: 20)
 *   --detail <span>     Show individual calls for a specific span
 *   --all               Include internal/library spans
 */

import { readFileSync } from "fs";

interface SpanEvent {
  timestamp: string;
  level: string;
  fields: {
    message: string;
    "time.busy"?: string;
    "time.idle"?: string;
  };
  target: string;
  span?: {
    name: string;
    [key: string]: unknown;
  };
  spans?: Array<{ name: string; [key: string]: unknown }>;
}

interface SpanStats {
  name: string;
  calls: number;
  busyTimes: number[]; // in microseconds
  idleTimes: number[];
  parentCounts: Map<string, number>; // parent span name -> count
}

// Parse duration strings like "1.23ms", "456µs", "789ns" to microseconds
function parseDuration(duration: string): number {
  const match = duration.match(/^([\d.]+)(ns|µs|us|ms|s)$/);
  if (!match) return 0;

  const value = parseFloat(match[1]);
  const unit = match[2];

  switch (unit) {
    case "ns":
      return value / 1000;
    case "µs":
    case "us":
      return value;
    case "ms":
      return value * 1000;
    case "s":
      return value * 1_000_000;
    default:
      return 0;
  }
}

// Format microseconds for display
function formatDuration(us: number): string {
  if (us < 1) {
    return `${(us * 1000).toFixed(0)}ns`;
  } else if (us < 1000) {
    return `${us.toFixed(2)}µs`;
  } else if (us < 1_000_000) {
    return `${(us / 1000).toFixed(2)}ms`;
  } else {
    return `${(us / 1_000_000).toFixed(2)}s`;
  }
}

function percentile(arr: number[], p: number): number {
  if (arr.length === 0) return 0;
  const sorted = [...arr].sort((a, b) => a - b);
  const idx = Math.floor(sorted.length * p);
  return sorted[Math.min(idx, sorted.length - 1)];
}

function parseJsonLines(content: string): SpanEvent[] {
  const events: SpanEvent[] = [];
  for (const line of content.trim().split("\n")) {
    if (!line.trim()) continue;
    try {
      events.push(JSON.parse(line));
    } catch {
      // Skip malformed lines
    }
  }
  return events;
}

function main() {
  const args = process.argv.slice(2);

  // Parse arguments
  let filterPattern: RegExp | null = null;
  let sortField = "total";
  let topN = 20;
  let detailSpan: string | null = null;
  let showAll = false;
  const files: string[] = [];

  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--filter" && args[i + 1]) {
      filterPattern = new RegExp(args[++i]);
    } else if (args[i] === "--sort" && args[i + 1]) {
      sortField = args[++i];
    } else if (args[i] === "--top" && args[i + 1]) {
      topN = parseInt(args[++i], 10);
    } else if (args[i] === "--detail" && args[i + 1]) {
      detailSpan = args[++i];
    } else if (args[i] === "--all") {
      showAll = true;
    } else if (!args[i].startsWith("-")) {
      files.push(args[i]);
    }
  }

  if (files.length === 0) {
    console.error("Usage: bun scripts/span-table.ts <file.json> [options]");
    console.error("  --filter <pattern>  Only show spans matching pattern (regex)");
    console.error("  --sort <field>      Sort by: calls, avg, total, p99 (default: total)");
    console.error("  --top <n>           Show top N spans (default: 20)");
    console.error("  --detail <span>     Show individual calls for a specific span");
    console.error("  --all               Include internal/library spans");
    process.exit(1);
  }

  // Parse all files
  const allEvents: SpanEvent[] = [];
  for (const file of files) {
    const content = readFileSync(file, "utf-8");
    for (const event of parseJsonLines(content)) {
      allEvents.push(event);
    }
  }

  // Filter to close events and aggregate by span name
  const spans = new Map<string, SpanStats>();

  for (const event of allEvents) {
    if (event.fields?.message !== "close") continue;
    if (!event.span?.name) continue;
    if (!event.fields["time.busy"]) continue;

    const name = event.span.name;

    // Apply filter if specified
    if (filterPattern && !filterPattern.test(name)) continue;

    // Skip noisy internal spans unless explicitly requested
    if (
      !showAll &&
      !filterPattern &&
      !detailSpan &&
      (name.startsWith("FramedRead::") ||
        name.startsWith("FramedWrite::") ||
        name.startsWith("Prioritize::") ||
        name === "poll" ||
        name === "poll_ready" ||
        name === "Connection" ||
        name.startsWith("assign_") ||
        name.startsWith("reserve_") ||
        name.startsWith("try_") ||
        name.startsWith("send_") ||
        name.startsWith("pop_"))
    ) {
      continue;
    }

    if (!spans.has(name)) {
      spans.set(name, { name, calls: 0, busyTimes: [], idleTimes: [], parentCounts: new Map() });
    }

    const stats = spans.get(name)!;
    stats.calls++;
    stats.busyTimes.push(parseDuration(event.fields["time.busy"]));
    if (event.fields["time.idle"]) {
      stats.idleTimes.push(parseDuration(event.fields["time.idle"]));
    }

    // Track parent relationship (immediate parent is the last element in spans array)
    const parents = event.spans || [];
    const parentName = parents.length > 0 ? parents[parents.length - 1].name : "__root__";
    stats.parentCounts.set(parentName, (stats.parentCounts.get(parentName) || 0) + 1);
  }

  if (spans.size === 0) {
    console.error("No matching span close events found");
    process.exit(1);
  }

  // Detail mode: show individual calls for a specific span
  if (detailSpan) {
    const detailEvents: Array<{
      timestamp: string;
      busy: number;
      idle: number;
      fields: Record<string, unknown>;
      parents: string[];
    }> = [];

    for (const event of allEvents) {
      if (event.fields?.message !== "close") continue;
      if (event.span?.name !== detailSpan) continue;
      if (!event.fields["time.busy"]) continue;

      // Extract span fields (excluding name)
      const fields: Record<string, unknown> = {};
      if (event.span) {
        for (const [k, v] of Object.entries(event.span)) {
          if (k !== "name") fields[k] = v;
        }
      }

      // Get parent span names
      const parents = (event.spans || []).map((s) => s.name);

      detailEvents.push({
        timestamp: event.timestamp,
        busy: parseDuration(event.fields["time.busy"]),
        idle: event.fields["time.idle"] ? parseDuration(event.fields["time.idle"]) : 0,
        fields,
        parents,
      });
    }

    if (detailEvents.length === 0) {
      console.error(`No events found for span "${detailSpan}"`);
      process.exit(1);
    }

    console.log("");
    console.log(`Individual calls for: ${detailSpan}`);
    console.log("-".repeat(110));
    console.log(
      "#".padStart(4) +
        "Wall".padStart(12) +
        "Busy".padStart(12) +
        "Idle".padStart(12) +
        "  Fields"
    );
    console.log("-".repeat(110));

    detailEvents.forEach((e, i) => {
      const fieldsStr = Object.keys(e.fields).length > 0
        ? JSON.stringify(e.fields)
        : "";

      console.log(
        (i + 1).toString().padStart(4) +
          formatDuration(e.busy + e.idle).padStart(12) +
          formatDuration(e.busy).padStart(12) +
          formatDuration(e.idle).padStart(12) +
          "  " +
          fieldsStr
      );
    });

    // Summary stats
    const busyTimes = detailEvents.map((e) => e.busy);
    const wallTimes = detailEvents.map((e) => e.busy + e.idle);
    console.log("");
    console.log(
      `Summary: ${detailEvents.length} calls\n` +
        `  Wall: avg=${formatDuration(wallTimes.reduce((a, b) => a + b, 0) / wallTimes.length)}, ` +
        `min=${formatDuration(Math.min(...wallTimes))}, ` +
        `max=${formatDuration(Math.max(...wallTimes))}, ` +
        `p50=${formatDuration(percentile(wallTimes, 0.5))}, ` +
        `p99=${formatDuration(percentile(wallTimes, 0.99))}\n` +
        `  Busy: avg=${formatDuration(busyTimes.reduce((a, b) => a + b, 0) / busyTimes.length)}, ` +
        `min=${formatDuration(Math.min(...busyTimes))}, ` +
        `max=${formatDuration(Math.max(...busyTimes))}, ` +
        `p50=${formatDuration(percentile(busyTimes, 0.5))}, ` +
        `p99=${formatDuration(percentile(busyTimes, 0.99))}`
    );
    return;
  }

  // Calculate stats
  const results = [...spans.values()].map((s) => {
    // Calculate wall times (busy + idle) for each call
    const wallTimes = s.busyTimes.map((busy, i) => busy + (s.idleTimes[i] || 0));

    // Find most common parent
    let mostCommonParent = "__root__";
    let maxCount = 0;
    for (const [parent, count] of s.parentCounts) {
      if (count > maxCount) {
        maxCount = count;
        mostCommonParent = parent;
      }
    }

    return {
      name: s.name,
      calls: s.calls,
      total: s.busyTimes.reduce((a, b) => a + b, 0),
      avg: s.busyTimes.reduce((a, b) => a + b, 0) / s.calls,
      min: Math.min(...s.busyTimes),
      max: Math.max(...s.busyTimes),
      p50: percentile(s.busyTimes, 0.5),
      p99: percentile(s.busyTimes, 0.99),
      avgWall: wallTimes.reduce((a, b) => a + b, 0) / s.calls,
      p50Wall: percentile(wallTimes, 0.5),
      p99Wall: percentile(wallTimes, 0.99),
      parent: mostCommonParent,
    };
  });

  // Build tree structure
  const childrenOf = new Map<string, string[]>();
  childrenOf.set("__root__", []);
  for (const r of results) {
    if (!childrenOf.has(r.name)) {
      childrenOf.set(r.name, []);
    }
    if (!childrenOf.has(r.parent)) {
      childrenOf.set(r.parent, []);
    }
    childrenOf.get(r.parent)!.push(r.name);
  }

  // Sort children by the specified field
  const resultMap = new Map(results.map(r => [r.name, r]));
  const sortChildren = (children: string[]) => {
    children.sort((a, b) => {
      const ra = resultMap.get(a);
      const rb = resultMap.get(b);
      if (!ra || !rb) return 0;
      switch (sortField) {
        case "calls":
          return rb.calls - ra.calls;
        case "avg":
          return rb.avg - ra.avg;
        case "p99":
          return rb.p99 - ra.p99;
        case "total":
        default:
          return rb.total - ra.total;
      }
    });
  };

  // Traverse tree to build ordered display list with depths
  const displayResults: Array<{ result: typeof results[0]; depth: number }> = [];
  const visited = new Set<string>();

  function traverse(name: string, depth: number) {
    if (visited.has(name)) return;
    visited.add(name);

    const result = resultMap.get(name);
    if (result) {
      displayResults.push({ result, depth });
    }

    const children = childrenOf.get(name) || [];
    sortChildren(children);
    for (const child of children) {
      traverse(child, depth + 1);
    }
  }

  // Start from roots
  const roots = childrenOf.get("__root__") || [];
  sortChildren(roots);
  for (const root of roots) {
    traverse(root, 0);
  }

  // Add any orphaned spans (whose parent wasn't in our span list)
  for (const r of results) {
    if (!visited.has(r.name)) {
      displayResults.push({ result: r, depth: 0 });
    }
  }

  // Apply topN limit
  const limitedResults = displayResults.slice(0, topN);

  console.log("");
  console.log(
    "Span Name".padEnd(40) +
      "Calls".padStart(6) +
      "Avg(wall)".padStart(11) +
      "P50(wall)".padStart(11) +
      "P99(wall)".padStart(11) +
      "Avg(busy)".padStart(11) +
      "P50(busy)".padStart(11) +
      "P99(busy)".padStart(11)
  );
  console.log("-".repeat(112));

  for (const { result: r, depth } of limitedResults) {
    const indent = "  ".repeat(depth);
    const maxNameLen = 38 - indent.length;
    const truncatedName = r.name.length > maxNameLen ? "..." + r.name.slice(-(maxNameLen - 3)) : r.name;
    const displayName = indent + truncatedName;

    console.log(
      displayName.padEnd(40) +
        r.calls.toString().padStart(6) +
        formatDuration(r.avgWall).padStart(11) +
        formatDuration(r.p50Wall).padStart(11) +
        formatDuration(r.p99Wall).padStart(11) +
        formatDuration(r.avg).padStart(11) +
        formatDuration(r.p50).padStart(11) +
        formatDuration(r.p99).padStart(11)
    );
  }

  console.log("");
  console.log(`Showing ${limitedResults.length} of ${results.length} spans (sorted by ${sortField})`);
}

main();
