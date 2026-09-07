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
 *
 * Dual V1 + V2 form:
 * - V1 (opencode 1.x) calls `server()` and uses the returned hooks.
 * - V2 (opencode2 beta) reads the default export's `id` and `setup()`.
 * The named `AtuinPlugin` export is kept for pre-1.18.29 V1 releases that
 * expect bare function exports.
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
	// V2 only: proposal id that claimed this entry. Lets `finish` close only
	// its own entry, so a denied or concurrent duplicate command can never
	// steal another call's history entry or exit code.
	owner?: string;
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

// V1 factory. Kept as a named export for pre-1.18.29 releases that expect
// bare function exports; 1.18.29+ uses it via the default export's `server`.
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

// V2 `tool.execute.after` reports `{ status: "completed", result }` or
// `{ status: "error", error }` instead of V1's `{ output, metadata }`.
// Shapes differ between beta builds, so read defensively and never throw.
function v2ExitCodeFrom(event: unknown): number {
	const e = (event ?? {}) as {
		status?: unknown;
		result?: unknown;
		error?: unknown;
	};
	const payload = e.status === "error" ? e.error : e.result;
	const metadata = (payload as { metadata?: unknown } | undefined)?.metadata;
	const exit = (metadata as { exit?: unknown } | undefined)?.exit;
	if (typeof exit === "number") return exit;

	let text = "";
	try {
		const p = payload as { message?: unknown; output?: unknown } | undefined;
		if (typeof p?.message === "string") text = p.message;
		else if (typeof p?.output === "string") text = p.output;
		else text = JSON.stringify(payload ?? "");
	} catch {
		text = "";
	}
	if (text.includes("User aborted the command")) return EXIT_ABORTED;
	if (/exceeding timeout \d+ ms/.test(text)) return EXIT_TIMED_OUT;
	return e.status === "completed" ? 0 : 1;
}

function splitProposal(args: unknown): Proposal | undefined {
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

// V2 setup (opencode2 beta). Same lifecycle as V1, adapted to the V2 API:
// - `tool.execute.before` remembers the proposal (no Atuin call yet, so
//   denied commands stay unrecorded).
// - `shell.create.before` is the first post-permission hook carrying the
//   resolved cwd, so it opens the history entry. It carries no call ID, so
//   the pending proposal is claimed by matching `command`.
// - `tool.execute.after` closes the entry; it also drops unclaimed
//   proposals, so denied commands never reach Atuin.
// `ctx` is typed loosely on purpose: the file must also load under V1
// runtimes whose `@opencode-ai/plugin` package predates the V2 `tool`/`shell`
// domains. Missing domains disable the V2 path quietly instead of crashing.
async function atuinSetup(
	ctx: any,
): Promise<(() => void | Promise<void>) | void> {
	const tool = ctx?.tool;
	const shell = ctx?.shell;
	if (typeof tool?.hook !== "function" || typeof shell?.hook !== "function") {
		return;
	}
	const baseDir =
		typeof ctx?.location?.directory === "string"
			? (ctx.location.directory as string)
			: undefined;

	// Proposals keyed by V2 tool call id.
	const proposed = new Map<string, Proposal>();
	// Open history entries keyed by command (the only key the shell hook and
	// the tool hook share), each tagged with the claiming proposal id.
	// `finish` closes only the entry owned by its own call id: a denied or
	// concurrent duplicate can neither steal another call's entry nor
	// misattribute an exit code. At most an entry goes unclosed, matching
	// the V1 eviction policy for commands that never report back.
	const running = new Map<string, Entry[]>();

	function rememberV2(callID: string, args: unknown) {
		const proposal = splitProposal(args);
		if (!proposal) return;

		if (proposed.size >= MAX_PROPOSED) {
			const oldest = proposed.keys().next().value;
			if (oldest !== undefined) proposed.delete(oldest);
		}

		proposed.set(callID, proposal);
	}

	async function claim(command: string, cwd: string | undefined) {
		for (const [id, proposal] of proposed) {
			if (proposal.command !== command) continue;
			proposed.delete(id);
			const historyId = await startHistory(cwd || baseDir || "", proposal);
			if (!historyId) return;
			const list = running.get(command) ?? [];
			list.push({ historyId, cwd: cwd || "", owner: id });
			running.set(command, list);
			return;
		}
	}

	async function finishV2(id: string, input: unknown, event: unknown) {
		const byId = proposed.get(id);
		proposed.delete(id);
		const command =
			byId?.command ?? (input as { command?: unknown } | undefined)?.command;
		if (typeof command !== "string" || command.length === 0) return;

		// Close only the entry this call claimed. Anything else (a denied
		// duplicate, an evicted proposal) leaves other calls' entries alone.
		const list = running.get(command);
		const index = list?.findIndex((entry) => entry.owner === id) ?? -1;
		if (!list || index < 0) return;
		const [entry] = list.splice(index, 1);
		if (list.length === 0) running.delete(command);
		if (!entry) return;

		await atuin(
			[
				"history",
				"end",
				entry.historyId,
				"--exit",
				String(v2ExitCodeFrom(event)),
			],
			entry.cwd,
		);
	}

	const registrations: unknown[] = [];
	try {
		registrations.push(
			await tool.hook("execute.before", (event: any) =>
				swallowFailures(() => {
					if (event?.tool !== BASH_TOOL) return;
					if (typeof event?.id !== "string") return;
					rememberV2(event.id, event.input);
				}),
			),
		);
		registrations.push(
			await shell.hook("create.before", (event: any) =>
				swallowFailures(() => {
					if (typeof event?.command !== "string" || event.command.length === 0) {
						return;
					}
					return claim(event.command, event.cwd);
				}),
			),
		);
		registrations.push(
			await tool.hook("execute.after", (event: any) =>
				swallowFailures(() => {
					if (event?.tool !== BASH_TOOL) return;
					if (typeof event?.id !== "string") return;
					return finishV2(event.id, event?.input, event);
				}),
			),
		);
	} catch {
		// Hook registration failed: stay unloaded instead of failing plugin
		// load. A missing history entry is always the better failure.
		return;
	}

	return async () => {
		for (const r of registrations) {
			try {
				await (r as { dispose?: () => unknown } | undefined)?.dispose?.();
			} catch {
				// Deliberately ignored.
			}
		}
	};
}

// Dual V1 + V2 entrypoint: V1 calls `server()`, V2 reads `id` + `setup()`.
export default {
	id: "atuin",
	setup: atuinSetup,
	server: AtuinPlugin,
};
