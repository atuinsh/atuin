/**
 * Atuin plugin for Amp.
 *
 * Amp keeps its threads on its servers, so at the end of every turn this
 * converts the turn's messages to Atuin's format and pushes them into
 * Threads with `atuin threads push` (see `atuin threads push --help`).
 *
 * Install with:
 *   atuin hook install amp
 *
 * Then restart Amp.
 */

import type { PluginAPI } from "@ampcode/plugin";
import { spawn } from "node:child_process";

type Block = Record<string, any>;

// A shell command that ran and failed is `done` too: only its exit code says.
function failed(output: unknown): boolean {
	if (typeof output !== "string") return false;
	try {
		const code = JSON.parse(output)?.exitCode;
		return typeof code === "number" && code !== 0;
	} catch {
		return false;
	}
}

// Images are left out of a tool's output: their bytes only.
function withoutImageBytes(output: unknown): unknown {
	if (!Array.isArray(output)) return output ?? null;
	return output.map((b) => (b?.type === "image" ? { ...b, data: undefined } : b));
}

function block(b: Block): Block {
	switch (b.type) {
		case "text":
			return { type: "text", text: b.text };
		case "thinking":
			return { type: "reasoning", text: b.thinking };
		case "tool_use":
			return { type: "tool_use", id: b.id, name: b.name, input: b.input };
		case "tool_result":
			return {
				type: "tool_result",
				id: b.toolUseID,
				output: withoutImageBytes(b.output),
				error: b.status !== "done" || failed(b.output),
			};
		default:
			return b;
	}
}

// How long a push may take: a turn it misses is left out of Threads, not held up.
const PUSH_TIMEOUT_MS = 10_000;

const STOP: Record<string, string> = { end_turn: "end_turn", tool_use: "tool_use", cancelled: "aborted" };

export default function (amp: PluginAPI) {
	amp.on("agent.end", async (event, ctx) => {
		try {
			const root = amp.system.workspaceRoot;
			const cwd = root ? amp.helpers.filePathFromURI(root) : undefined;
			// The message before the turn, which its first follows.
			const recent = await ctx.thread.messages({ from: "end", limit: event.messages.length + 1 });
			let parent = recent.length > event.messages.length ? recent[0].id : undefined;
			const lines = event.messages.map((m: Block) => {
				const usage = m.usage;
				const line = {
					v: 1,
					id: m.id,
					parent,
					role: m.role === "info" ? "system" : m.role,
					time: m.createdAt,
					content: (m.content ?? []).map(block),
					cwd,
					model: usage?.model,
					turn: usage ? m.id : undefined,
					usage: usage && {
						input: usage.inputTokens,
						output: usage.outputTokens,
						cache_read: usage.cacheReadInputTokens ?? undefined,
						cache_write: usage.cacheCreationInputTokens ?? undefined,
					},
					stop: m.stopReason && (STOP[m.stopReason] ?? m.stopReason),
				};
				parent = m.id;
				return `${JSON.stringify(line)}\n`;
			});
			const push = spawn("atuin", ["threads", "push", "--harness", "amp", "--session", event.thread.id], {
				stdio: ["pipe", "ignore", "ignore"],
			});
			const done = new Promise((resolve) => {
				const timer = setTimeout(() => {
					push.kill();
					resolve(undefined);
				}, PUSH_TIMEOUT_MS);
				const settle = () => {
					clearTimeout(timer);
					resolve(undefined);
				};
				push.on("close", settle);
				push.on("error", settle);
			});
			push.stdin.on("error", () => {});
			push.stdin.end(lines.join(""));
			// Amp stops the plugin once the turn's handlers return.
			await done;
		} catch {
			// Never get in Amp's way.
		}
	});
}
