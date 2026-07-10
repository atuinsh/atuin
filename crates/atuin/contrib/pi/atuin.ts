/**
 * Atuin extension for pi.
 *
 * Tracks bash commands executed by pi in Atuin history with author `pi`.
 *
 * Install with:
 *   atuin hook install pi
 *
 * Then restart pi or run /reload.
 */

import type { ExtensionAPI, ExtensionContext } from "@mariozechner/pi-coding-agent";

const ATUIN_AUTHOR = "pi";
const ATUIN_TIMEOUT_MS = 10_000;

async function startHistory(
	pi: ExtensionAPI,
	cwd: string,
	command: string,
): Promise<string | undefined> {
	try {
		const result = await pi.exec(
			"atuin",
			["history", "start", "--author", ATUIN_AUTHOR, "--", command],
			{ cwd, timeout: ATUIN_TIMEOUT_MS },
		);

		if (result.code !== 0) return undefined;

		const id = result.stdout.trim();
		return id.length > 0 ? id : undefined;
	} catch {
		return undefined;
	}
}

async function endHistory(
	pi: ExtensionAPI,
	cwd: string,
	historyId: string,
	exitCode: number,
): Promise<void> {
	try {
		await pi.exec(
			"atuin",
			["history", "end", historyId, "--exit", String(exitCode)],
			{ cwd, timeout: ATUIN_TIMEOUT_MS },
		);
	} catch {
		// Ignore Atuin failures so command execution is never blocked.
	}
}

// The bash tool reports failures by appending a status line to the result
// text rather than exposing a numeric exit code, so recover it from there.
function exitCodeFromResult(result: unknown, isError: boolean): number {
	if (!isError) return 0;

	const content = (result as { content?: unknown } | undefined)?.content;
	const text = Array.isArray(content)
		? content
				.map((part) => {
					const t = (part as { text?: unknown } | undefined)?.text;
					return typeof t === "string" ? t : "";
				})
				.join("\n")
		: "";

	const exited = text.match(/Command exited with code (\d+)\s*$/);
	if (exited) return Number(exited[1]);
	if (/Command aborted\s*$/.test(text)) return 130;
	if (/Command timed out after \S+ seconds\s*$/.test(text)) return 124;
	return 1;
}

export default function atuinPiExtension(pi: ExtensionAPI) {
	// Atuin history IDs for in-flight bash tool calls, keyed by tool call ID.
	const pending = new Map<string, string>();

	// Observe bash executions through events instead of registering a bash
	// tool: registering one conflicts with other extensions that provide
	// their own bash tool (sandboxes, RTK, remote runners), while events
	// fire no matter which extension's bash tool ends up executing the
	// command.
	pi.on("tool_call", async (event, ctx: ExtensionContext) => {
		if (event.toolName !== "bash") return;

		const command = (event.input as { command?: unknown }).command;
		if (typeof command !== "string" || command.length === 0) return;

		const historyId = await startHistory(pi, ctx.cwd, command);
		if (historyId) pending.set(event.toolCallId, historyId);
	});

	// tool_execution_end also fires when another extension blocks the call,
	// unlike tool_result, so entries started above are always closed.
	pi.on("tool_execution_end", async (event, ctx: ExtensionContext) => {
		const historyId = pending.get(event.toolCallId);
		if (!historyId) return;
		pending.delete(event.toolCallId);

		await endHistory(pi, ctx.cwd, historyId, exitCodeFromResult(event.result, event.isError));
	});
}
