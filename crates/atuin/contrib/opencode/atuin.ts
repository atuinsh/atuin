/**
 * Atuin plugin for opencode V1, including legacy function-only loaders.
 *
 * Install with `atuin hook install opencode`, then restart opencode.
 * For the V2 plugin API, use `atuin hook install opencode-v2` instead.
 * Both installers write the same plugin path; install only one variant.
 */

import type { Plugin } from "@opencode-ai/plugin";
import { spawn } from "node:child_process";

const ATUIN_AUTHOR = "opencode";
const ATUIN_TIMEOUT_MS = 10_000;
const BASH_TOOL = "bash";
const EXIT_ABORTED = 130;
const EXIT_TIMED_OUT = 124;

// Denied commands never reach shell.env or tool.execute.after.
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

/** Run without a shell: the recorded command must stay a single argv entry. */
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
	const args = ["history", "start", "--author", ATUIN_AUTHOR, "--author-kind", "agent"];
	if (proposal.intent) args.push("--intent", proposal.intent);
	args.push("--", proposal.command);
	const result = await atuin(args, cwd);
	if (result.code !== 0) return undefined;
	const historyId = result.stdout.trim();
	return historyId.length > 0 ? historyId : undefined;
}

function exitCodeFrom(output: ToolOutput): number {
	const exit = (output.metadata as { exit?: unknown } | undefined)?.exit;
	if (typeof exit === "number") return exit;
	const text = typeof output.output === "string" ? output.output : "";
	if (text.includes("User aborted the command")) return EXIT_ABORTED;
	if (/exceeding timeout \d+ ms/.test(text)) return EXIT_TIMED_OUT;
	return 1;
}

// Recording history must not abort a tool call or discard its output.
async function swallowFailures(work: () => void | Promise<void>): Promise<void> {
	try {
		await work();
	} catch {
		// A missing history entry is always the better failure.
	}
}

const AtuinPlugin: Plugin = async ({ directory }) => {
	const proposed = new Map<string, Proposal>();
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
			["history", "end", entry.historyId, "--exit", String(exitCodeFrom(output))],
			entry.cwd,
		);
	}

	return {
		// Before permission: remember only, so denied commands stay unrecorded.
		"tool.execute.before": (input, output) =>
			swallowFailures(() => {
				if (input.tool !== BASH_TOOL) return;
				propose(input.callID, output.args);
			}),
		// After permission, with the resolved cwd and exact tool call ID.
		"shell.env": (input) =>
			swallowFailures(() => {
				if (!input.callID) return;
				return start(input.callID, input.cwd || directory);
			}),
		"tool.execute.after": (input, output) =>
			swallowFailures(() => {
				if (input.tool !== BASH_TOOL) return;
				return finish(input.callID, output);
			}),
	};
};

// Export exactly one function. Older V1 loaders invoke every module export.
export default AtuinPlugin;
