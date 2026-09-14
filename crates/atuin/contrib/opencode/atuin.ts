/**
 * Atuin plugin for opencode.
 *
 * Tracks bash commands executed by opencode in Atuin history with author
 * `opencode`.
 *
 * Install with:
 *   atuin hook install opencode
 *
 * Then make sure the plugin SDK is installed:
 *
 *   npm --prefix ~/.config/opencode install --save-exact @opencode/plugin
 *
 * Then restart opencode.
 *
 * This file supports both opencode V1 (>= 1.18.29) and V2 from a single
 * default export: V1 calls `server()` and uses the returned hooks, while V2
 * reads the `id` and `setup()` definition and ignores `server()`.
 * See https://opencode.ai/v2/docs/build/plugins/migrate-v1.
 */

import { Plugin as V2Plugin } from "@opencode/plugin";
import type { Plugin as V1Plugin } from "@opencode-ai/plugin";
import { spawn } from "node:child_process";

const ATUIN_AUTHOR = "opencode";
const ATUIN_TIMEOUT_MS = 10_000;

// opencode's shell tool kept the id `bash` for compatibility for a long time;
// newer releases call it `shell`. The V2 hooks accept either; the V1 hooks
// keep the original `bash` check.
const SHELL_TOOLS = new Set(["bash", "shell"]);

// A command that did not exit on its own reports a null exit code, so
// substitute the conventional code for each of the two ways that happens.
const EXIT_ABORTED = 130;
const EXIT_TIMED_OUT = 124;

// A denied command is proposed but never runs, so its proposal is never
// claimed and would sit in the map for the life of the session. Bound the map
// to cap that. Evicting the oldest can in principle drop a command still
// waiting at its permission prompt, which just goes unrecorded, but only a
// handful of calls are ever genuinely in flight at once.
const MAX_PROPOSED = 128;

interface Proposal {
	command: string;
	intent?: string;
}

interface Entry {
	historyId: string;
	cwd: string;
}

interface AtuinResult {
	code: number | null;
	stdout: string;
}

/**
 * Run Atuin, resolving rather than rejecting on every failure.
 *
 * Spawned without a shell: a recorded command is arbitrary text that has to
 * reach Atuin as a single argv entry unmangled.
 */
function atuin(args: string[], cwd: string): Promise<AtuinResult> {
	return new Promise((resolve) => {
		let child: ReturnType<typeof spawn>;
		try {
			child = spawn("atuin", args, { cwd, stdio: ["ignore", "pipe", "ignore"] });
		} catch {
			resolve({ code: null, stdout: "" });
			return;
		}

		let stdout = "";
		let settled = false;
		const timer = setTimeout(() => child.kill("SIGKILL"), ATUIN_TIMEOUT_MS);

		const settle = (result: AtuinResult) => {
			if (settled) return;
			settled = true;
			clearTimeout(timer);
			resolve(result);
		};

		child.stdout?.setEncoding("utf8");
		child.stdout?.on("data", (chunk: string) => {
			stdout += chunk;
		});
		child.on("error", () => settle({ code: null, stdout: "" }));
		child.on("close", (code) => settle({ code, stdout }));
	});
}

async function startHistory(
	cwd: string,
	proposal: Proposal,
): Promise<string | undefined> {
	const args = [
		"history",
		"start",
		"--author",
		ATUIN_AUTHOR,
		"--author-kind",
		"agent",
	];
	if (proposal.intent) args.push("--intent", proposal.intent);
	args.push("--", proposal.command);

	const result = await atuin(args, cwd);
	if (result.code !== 0) return undefined;

	const historyId = result.stdout.trim();
	return historyId.length > 0 ? historyId : undefined;
}

function toProposal(args: unknown): Proposal | undefined {
	const { command, description } = (args ?? {}) as {
		command?: unknown;
		description?: unknown;
	};
	if (typeof command !== "string" || command.length === 0) return undefined;
	return {
		command,
		intent:
			typeof description === "string" && description.length > 0
				? description
				: undefined,
	};
}

function rememberProposal(proposed: Map<string, Proposal>, callID: string, args: unknown) {
	const proposal = toProposal(args);
	if (!proposal) return;

	if (proposed.size >= MAX_PROPOSED) {
		const oldest = proposed.keys().next().value;
		if (oldest !== undefined) proposed.delete(oldest);
	}

	proposed.set(callID, proposal);
}

function exitFromMetadata(metadata: unknown): number | undefined {
	const exit = (metadata as { exit?: unknown } | undefined)?.exit;
	return typeof exit === "number" ? exit : undefined;
}

// The tool records an abort or a timeout in its output text rather than as an
// exit code, so string matching is the only way to tell the two apart.
function exitCodeFromText(text: unknown): number {
	if (typeof text !== "string") return 1;
	if (text.includes("User aborted the command")) return EXIT_ABORTED;
	if (/exceeding timeout \d+ ms/.test(text)) return EXIT_TIMED_OUT;
	return 1;
}

// A rejected hook aborts opencode's tool call and discards the command's
// output. A missing history entry is always the better failure.
async function swallowFailures(work: () => void | Promise<void>): Promise<void> {
	try {
		await work();
	} catch {
		// Deliberately ignored.
	}
}

const v2 = V2Plugin.define({
	id: "atuin",
	async setup(ctx) {
		// Commands opencode has proposed but that have not started yet, keyed
		// by tool call ID.
		const proposed = new Map<string, Proposal>();
		// Atuin history IDs for commands that did start, keyed by tool call ID.
		const running = new Map<string, Entry>();

		// Unlike V1's `shell.env`, the V2 `create.before` shell hook carries
		// no tool call ID, only the command about to run and its resolved
		// working directory. Claim the oldest pending proposal with the same
		// command text. Shells opencode did not run for a tool call have no
		// matching proposal and are skipped.
		async function claim(command: string, cwd: string) {
			for (const [callID, proposal] of proposed) {
				if (proposal.command !== command) continue;
				proposed.delete(callID);

				const historyId = await startHistory(cwd, proposal);
				if (historyId) running.set(callID, { historyId, cwd });
				return;
			}
		}

		async function finish(
			callID: string,
			event:
				| { status: "completed"; result: { output?: unknown; metadata?: unknown } }
				| { status: "error"; error: { message: string; metadata?: unknown } },
		) {
			proposed.delete(callID);

			const entry = running.get(callID);
			if (!entry) return;
			running.delete(callID);

			const exit =
				event.status === "completed"
					? (exitFromMetadata(event.result.metadata) ??
						exitCodeFromText(event.result.output))
					: (exitFromMetadata(event.error.metadata) ??
						exitCodeFromText(event.error.message));

			await atuin(
				["history", "end", entry.historyId, "--exit", String(exit)],
				entry.cwd,
			);
		}

		// Fires when the tool is invoked, before it runs, so only remember
		// the command here. Starting an entry would record commands the user
		// went on to deny.
		await ctx.tool.hook("execute.before", (event) =>
			swallowFailures(() => {
				if (!SHELL_TOOLS.has(event.tool)) return;
				rememberProposal(proposed, String(event.id), event.input);
			}),
		);

		// The only hook that runs after the permission prompt but before the
		// command does, and the only one given the resolved working directory.
		await ctx.shell.hook("create.before", (event) =>
			swallowFailures(() => claim(event.command, event.cwd)),
		);

		// Fires when the tool call completes or fails. Abort and timeout
		// surface here with a distinctive message but no usable exit code.
		await ctx.tool.hook("execute.after", (event) =>
			swallowFailures(() => {
				if (!SHELL_TOOLS.has(event.tool)) return;
				if (event.status === "completed") {
					const result = event.result as {
						output?: unknown;
						metadata?: unknown;
					};
					return finish(String(event.id), { status: "completed", result });
				}
				const error = event.error as { message: string; metadata?: unknown };
				return finish(String(event.id), { status: "error", error });
			}),
		);
	},
});

// V1 implementation, kept for opencode 1.x. V1's `shell.env` hook carries the
// tool call ID, so the claim step is an exact lookup: the only hook that runs
// after the permission prompt but before the command does is also the only
// one given both the resolved working directory and the call ID. It also
// fires for user-run shells and for PTY sessions, which opencode did not run;
// those are skipped because they have no proposal to claim.
const server: V1Plugin = async ({ directory }) => {
	// Commands opencode has proposed but is not yet cleared to run, keyed by
	// tool call ID.
	const proposed = new Map<string, Proposal>();
	// Atuin history IDs for commands that did start, keyed by tool call ID.
	const running = new Map<string, Entry>();

	async function start(callID: string, cwd: string) {
		const proposal = proposed.get(callID);
		if (!proposal) return;
		proposed.delete(callID);

		const historyId = await startHistory(cwd, proposal);
		if (historyId) running.set(callID, { historyId, cwd });
	}

	async function finish(callID: string, output: { output: string; metadata: unknown }) {
		proposed.delete(callID);

		const entry = running.get(callID);
		if (!entry) return;
		running.delete(callID);

		await atuin(
			[
				"history",
				"end",
				entry.historyId,
				"--exit",
				String(
					exitFromMetadata(output.metadata) ?? exitCodeFromText(output.output),
				),
			],
			entry.cwd,
		);
	}

	return {
		// Fires before the permission prompt, so only remember the command here.
		// Starting an entry would record commands the user went on to deny.
		"tool.execute.before": (input, output) =>
			swallowFailures(() => {
				if (input.tool !== "bash") return;
				rememberProposal(proposed, input.callID, output.args);
			}),

		// The only hook that runs after the permission prompt but before the
		// command does, and the only one given the resolved working directory.
		// It also fires for user-run shells and for PTY sessions, which
		// opencode did not run; those are skipped because they have no
		// proposal to claim, not because of the call ID check below.
		"shell.env": (input) =>
			swallowFailures(() => {
				if (!input.callID) return;
				return start(input.callID, input.cwd || directory);
			}),

		// Also fires when the user aborts a command, unlike the error paths,
		// which skip this hook and leave the entry open.
		"tool.execute.after": (input, output) =>
			swallowFailures(() => {
				if (input.tool !== "bash") return;
				return finish(input.callID, output);
			}),
	};
};

export default {
	...v2,
	server,
};
