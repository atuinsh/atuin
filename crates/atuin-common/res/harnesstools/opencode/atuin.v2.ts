/**
 * Atuin plugin for opencode 2.
 *
 * Tracks shell commands executed by opencode in Atuin history with author
 * `opencode`. (opencode 1.x takes a plugin of another shape: `atuin hook
 * install opencode` installs the one for the opencode it finds.)
 *
 * Install with:
 *   atuin hook install opencode
 *
 * Then restart opencode.
 */

import { spawn } from "node:child_process";

const ATUIN_AUTHOR = "opencode";
const ATUIN_TIMEOUT_MS = 10_000;

const SHELL_TOOL = "shell";

// A command killed for running too long reports no exit code; record the
// conventional one.
const EXIT_TIMED_OUT = 124;

// A denied command never completes, so its proposal is never claimed and would
// sit in the map for the life of the session. Bound the map to cap that.
// Evicting the oldest can in principle drop a command still waiting at its
// permission prompt, which just goes unrecorded, but only a handful of calls
// are ever genuinely in flight at once.
const MAX_PROPOSED = 128;

interface Proposal {
	command: string;
	intent?: string;
	started?: Started;
}

interface Started {
	shell: ShellCreate;
	at: bigint;
}

interface AtuinResult {
	code: number | null;
	stdout: string;
}

// What opencode hands its tool hooks: the call (`id`) and its input, and after
// it ran, how it ended.
interface ToolCall {
	tool: string;
	id: string;
	input: unknown;
	status?: "completed" | "error";
	result?: { metadata?: { exit?: unknown; status?: unknown; timeout?: unknown } };
}

// What opencode hands `create.before`: the shell it is about to start. Its
// `cwd` is made absolute once the command is cleared to run.
interface ShellCreate {
	command: string;
	cwd: string;
}

interface Hooks {
	tool: {
		hook(name: string, fn: (call: ToolCall) => void | Promise<void>): unknown;
	};
	shell: {
		hook(name: string, fn: (shell: ShellCreate) => void | Promise<void>): unknown;
	};
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

// A rejected hook aborts opencode's tool call and discards the command's
// output. A missing history entry is always the better failure.
async function swallowFailures(work: () => void | Promise<void>): Promise<void> {
	try {
		await work();
	} catch {
		// Deliberately ignored.
	}
}

// The exit code a command that completed ended with: the shell tool completes
// a command whatever its exit code, and one it killed for running too long has
// none.
function exitCodeOf(call: ToolCall): number {
	const metadata = call.result?.metadata;
	if (typeof metadata?.exit === "number") return metadata.exit;
	if (metadata?.timeout === true) return EXIT_TIMED_OUT;
	return 1;
}

// opencode's hooks before a shell runs (`execute.before`, `create.before`) both
// come before the permission check, and none comes between that check and the
// command starting, so a command is recorded once it has ended: started then,
// and ended with the time since its shell was created. That time includes any
// wait at the permission prompt, and the entry's timestamp is when the command
// ended. A command that was denied ends the call in an error and is not
// recorded; so does one the user cancels, which is not recorded either. One
// left running in the background (`status: "running"`) has not ended, and is
// not recorded.
function setup(ctx: Hooks) {
	// Commands proposed, keyed by tool call ID.
	const proposed = new Map<string, Proposal>();

	ctx.tool.hook("execute.before", (call) =>
		swallowFailures(() => {
			if (call.tool !== SHELL_TOOL) return;
			const { command, description } = (call.input ?? {}) as {
				command?: unknown;
				description?: unknown;
			};
			if (typeof command !== "string" || command.length === 0) return;
			if (proposed.size >= MAX_PROPOSED) {
				const oldest = proposed.keys().next().value;
				if (oldest !== undefined) proposed.delete(oldest);
			}
			proposed.set(call.id, {
				command,
				intent:
					typeof description === "string" && description.length > 0
						? description
						: undefined,
			});
		}),
	);

	// Carries no call ID: it goes to the oldest proposal of the same command
	// whose shell has not been created yet.
	ctx.shell.hook("create.before", (shell) =>
		swallowFailures(() => {
			for (const proposal of proposed.values()) {
				if (proposal.started || proposal.command !== shell.command) continue;
				proposal.started = { shell, at: process.hrtime.bigint() };
				return;
			}
		}),
	);

	ctx.tool.hook("execute.after", (call) =>
		swallowFailures(async () => {
			if (call.tool !== SHELL_TOOL) return;
			const proposal = proposed.get(call.id);
			proposed.delete(call.id);
			if (!proposal?.started || call.status !== "completed") return;
			if (call.result?.metadata?.status === "running") return;

			const { shell, at } = proposal.started;
			const duration = process.hrtime.bigint() - at;
			const historyId = await startHistory(shell.cwd, proposal);
			if (!historyId) return;
			await atuin(
				[
					"history",
					"end",
					historyId,
					"--exit",
					String(exitCodeOf(call)),
					"--duration",
					String(duration),
				],
				shell.cwd,
			);
		}),
	);
}

// opencode 2 loads a plugin's default export: its `id` and `setup`.
export default { id: "atuin", setup };
