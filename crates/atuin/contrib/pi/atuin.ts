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

import type { BashOperations, ExtensionAPI } from "@mariozechner/pi-coding-agent";
import { createBashTool, createLocalBashOperations } from "@mariozechner/pi-coding-agent";

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

export default function atuinPiExtension(pi: ExtensionAPI) {
	const cwd = process.cwd();
	const local = createLocalBashOperations();

	const trackedOperations: BashOperations = {
		async exec(command, commandCwd, options) {
			const historyId = await startHistory(pi, commandCwd, command);
			let exitCode: number | null = null;

			try {
				const result = await local.exec(command, commandCwd, options);
				exitCode = result.exitCode;
				return result;
			} finally {
				if (historyId) {
					await endHistory(
						pi,
						commandCwd,
						historyId,
						exitCode ?? (options.signal?.aborted ? 130 : 1),
					);
				}
			}
		},
	};

	pi.registerTool(
		createBashTool(cwd, {
			operations: trackedOperations,
		}),
	);
}
