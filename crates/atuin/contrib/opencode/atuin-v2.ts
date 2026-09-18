/**
 * Atuin plugin for the OpenCode V2 tool/shell hook API.
 *
 * Install with `atuin hook install opencode-v2`, then restart OpenCode.
 * This replaces the V1 template at opencode/plugins/atuin.ts.
 * Do not place both templates in an autoload directory.
 */

import { spawn } from "node:child_process";

const ATUIN_AUTHOR = "opencode";
const ATUIN_TIMEOUT_MS = 10_000;
const BASH_TOOL = "bash";
const EXIT_ABORTED = 130;
const EXIT_TIMED_OUT = 124;

// Bound pending V2 proposals when the host omits completion events.
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

// V2 reports completion and error payloads instead of V1's ToolOutput.
function exitCodeFrom(event: unknown): number {
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
		// Malformed diagnostic payloads must not abort a tool call.
	}
	if (text.includes("User aborted the command")) return EXIT_ABORTED;
	if (/exceeding timeout \d+ ms/.test(text)) return EXIT_TIMED_OUT;
	return e.status === "completed" ? 0 : 1;
}

async function swallowFailures(work: () => void | Promise<void>): Promise<void> {
	try {
		await work();
	} catch {
		// A missing history entry is always the better failure.
	}
}

interface Pending extends Proposal {
	claimed: boolean;
	ambiguous: boolean;
}

// Beta hook domains are feature-detected rather than imported from the V1
// package. Builds without this API leave history recording disabled.
async function setup(ctx: any): Promise<(() => Promise<void>) | void> {
	const tool = ctx?.tool;
	const shell = ctx?.shell;
	if (typeof tool?.hook !== "function" || typeof shell?.hook !== "function") return;
	const baseDir =
		typeof ctx?.location?.directory === "string" ? ctx.location.directory : "";

	// Keep a proposal until its tool finishes, including after it is claimed.
	// Otherwise a concurrent duplicate could appear unique after the first
	// command started. History entries are always closed by exact tool ID.
	const proposed = new Map<string, Pending>();
	const running = new Map<string, Entry>();
	const registrations: { dispose?: () => unknown }[] = [];
	let active = false;
	let correlationDisabled = false;

	async function dispose() {
		// Make leftover callbacks inert even if a host disposer rejects.
		active = false;
		proposed.clear();
		running.clear();
		for (const registration of registrations.splice(0).reverse()) {
			await swallowFailures(async () => {
				await registration?.dispose?.();
			});
		}
	}

	function remember(id: string, args: unknown) {
		if (correlationDisabled || proposed.has(id)) return;
		const { command, description } = (args ?? {}) as {
			command?: unknown;
			description?: unknown;
		};
		if (typeof command !== "string" || command.length === 0) return;

		if (proposed.size >= MAX_PROPOSED) {
			// Eviction is unsafe without a shell call ID: a delayed, evicted
			// command could claim a newer identical proposal. Stop correlating
			// until reload, but still let already-started entries finish.
			correlationDisabled = true;
			proposed.clear();
			return;
		}

		let ambiguous = false;
		for (const proposal of proposed.values()) {
			if (proposal.command !== command) continue;
			proposal.ambiguous = true;
			ambiguous = true;
		}
		proposed.set(id, {
			command,
			intent: typeof description === "string" && description.length > 0
				? description : undefined,
			claimed: false,
			ambiguous,
		});
	}

	async function claim(command: string, cwd: unknown) {
		if (correlationDisabled) return;
		for (const [id, proposal] of proposed) {
			if (proposal.command !== command || proposal.claimed || proposal.ambiguous) continue;
			proposal.claimed = true;
			const resolvedCwd = typeof cwd === "string" && cwd.length > 0 ? cwd : baseDir;
			const historyId = await startHistory(resolvedCwd, proposal);
			// Setup may have been disposed while the Atuin subprocess ran.
			if (active && historyId) running.set(id, { historyId, cwd: resolvedCwd });
			return;
		}
	}

	async function finish(id: string, event: unknown) {
		proposed.delete(id);
		const entry = running.get(id);
		if (!entry) return;
		running.delete(id);
		await atuin(
			["history", "end", entry.historyId, "--exit", String(exitCodeFrom(event))],
			entry.cwd,
		);
	}

	try {
		registrations.push(await tool.hook("execute.before", (event: any) =>
			swallowFailures(() => {
				if (!active || event?.tool !== BASH_TOOL || typeof event?.id !== "string") return;
				remember(event.id, event.input);
			}),
		));
		registrations.push(await shell.hook("create.before", (event: any) =>
			swallowFailures(() => {
				if (!active || typeof event?.command !== "string" || !event.command) return;
				return claim(event.command, event.cwd);
			}),
		));
		registrations.push(await tool.hook("execute.after", (event: any) =>
			swallowFailures(() => {
				if (!active || event?.tool !== BASH_TOOL || typeof event?.id !== "string") return;
				return finish(event.id, event);
			}),
		));
		// No callback may open an entry until *all* hooks are registered.
		active = true;
	} catch {
		await dispose();
		return;
	}
	return dispose;
}

// V2 has an object-only definition; never expose it to legacy V1 loaders.
export default { id: "atuin", setup };
