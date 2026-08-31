/**
 * Atuin plugin for opencode.
 *
 * Tracks bash commands executed by opencode in Atuin history with author
 * `opencode`.
 *
 * Install with:
 *   atuin hook install opencode
 *
 * Then restart opencode.
 */

import type { Plugin } from "@opencode-ai/plugin";
import { spawn } from "node:child_process";

const ATUIN_AUTHOR = "opencode";
const ATUIN_TIMEOUT_MS = 10_000;

// opencode's shell tool keeps the id `bash` for compatibility, even though its
// module is named `shell`.
const BASH_TOOL = "bash";

// A command that did not exit on its own reports a null exit code, so
// substitute the conventional code for each of the two ways that happens.
const EXIT_ABORTED = 130;
const EXIT_TIMED_OUT = 124;

// A denied command reaches neither `shell.env` nor `tool.execute.after`, so its
// proposal is never claimed and would sit in the map for the life of the
// session. Bound the map to cap that. Evicting the oldest can in principle drop
// a command still waiting at its permission prompt, which just goes unrecorded,
// but only a handful of calls are ever genuinely in flight at once.
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

interface ToolOutput {
	output: string;
	metadata: unknown;
}

/**
 * Run Atuin, resolving rather than rejecting on every failure.
 *
 * Spawned without a shell: a recorded command is arbitrary text that has to
 * reach Atuin as a single argv entry unmangled, which rules out the Bun shell
 * the plugin API hands us.
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

// The tool records an abort or a timeout in its output text rather than as an
// exit code, so string matching is the only way to tell the two apart.
function exitCodeFrom(output: ToolOutput): number {
	const exit = (output.metadata as { exit?: unknown } | undefined)?.exit;
	if (typeof exit === "number") return exit;

	const text = typeof output.output === "string" ? output.output : "";
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

// opencode treats every export of a plugin file as a plugin function and fails
// to load the file if one is not, so keep this the only export.
export const AtuinPlugin: Plugin = async ({ directory }) => {
	// Commands opencode has proposed but is not yet cleared to run, keyed by
	// tool call ID.
	const proposed = new Map<string, Proposal>();
	// Atuin history IDs for commands that did start, keyed by tool call ID.
	const running = new Map<string, Entry>();

	function propose(callID: string, args: unknown) {
		const { command, description } = (args ?? {}) as {
			command?: unknown;
			description?: unknown;
		};
		if (typeof command !== "string" || command.length === 0) return;

		if (proposed.size >= MAX_PROPOSED) {
			const oldest = proposed.keys().next().value;
			if (oldest !== undefined) proposed.delete(oldest);
		}

		proposed.set(callID, {
			command,
			intent:
				typeof description === "string" && description.length > 0
					? description
					: undefined,
		});
	}

	async function start(callID: string, cwd: string) {
		const proposal = proposed.get(callID);
		if (!proposal) return;
		proposed.delete(callID);

		const historyId = await startHistory(cwd, proposal);
		if (historyId) running.set(callID, { historyId, cwd });
	}

	async function finish(callID: string, output: ToolOutput) {
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
				String(exitCodeFrom(output)),
			],
			entry.cwd,
		);
	}

	return {
		// Fires before the permission prompt, so only remember the command here.
		// Starting an entry would record commands the user went on to deny.
		"tool.execute.before": (input, output) =>
			swallowFailures(() => {
				if (input.tool !== BASH_TOOL) return;
				propose(input.callID, output.args);
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
				if (input.tool !== BASH_TOOL) return;
				return finish(input.callID, output);
			}),
	};
};
