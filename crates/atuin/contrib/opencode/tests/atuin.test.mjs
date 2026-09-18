import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import childProcess from "node:child_process";
import { syncBuiltinESMExports } from "node:module";
import { after, beforeEach, test } from "node:test";

// Exercise the installed, standalone templates without running a real Atuin
// binary or touching a user's history database. No npm dependencies required.
const originalSpawn = childProcess.spawn;
let calls;
let failure;
let nextId;
childProcess.spawn = (executable, args, options) => {
	assert.equal(executable, "atuin");
	calls.push({ args: [...args], cwd: options.cwd, options });
	if (failure === "throw") throw new Error("spawn failed");
	const child = new EventEmitter();
	child.stdout = new EventEmitter();
	child.stdout.setEncoding = () => {};
	child.kill = () => child.emit("close", null);
	queueMicrotask(() => {
		if (failure === "error") {
			child.emit("error", new Error("atuin missing"));
			child.emit("close", null);
			return;
		}
		if (args[1] === "start" && failure !== "empty") {
			child.stdout.emit("data", `history-${++nextId}\n`);
		}
		child.emit("close", failure === "nonzero" ? 1 : 0);
	});
	return child;
};
syncBuiltinESMExports();
const legacy = await import("../atuin.ts");
const v2 = await import("../atuin-v2.ts");
after(() => {
	childProcess.spawn = originalSpawn;
	syncBuiltinESMExports();
});
beforeEach(() => {
	calls = [];
	failure = undefined;
	nextId = 0;
});

function host({ failAt, disposeFailsAt, duringRegistration } = {}) {
	const callbacks = new Map();
	const disposed = [];
	let attempted = 0;
	function domain(name) {
		return {
			async hook(event, callback) {
				const index = ++attempted;
				callbacks.set(`${name}.${event}`, callback);
				await duringRegistration?.(callbacks, index);
				if (index === failAt) throw new Error("registration failed");
				return {
					async dispose() {
						disposed.push(index);
						if (index === disposeFailsAt) throw new Error("disposal failed");
					},
				};
			},
		};
	}
	return {
		ctx: { tool: domain("tool"), shell: domain("shell"), location: { directory: "/project" } },
		callbacks,
		disposed,
		emit: (name, event) => callbacks.get(name)?.(event),
	};
}
function before(id, command = "echo test", description = id) {
	return { tool: "bash", id, input: { command, description } };
}
function completed(id, exit = 0) {
	// No input.command is needed at completion: ownership is by exact ID.
	return { tool: "bash", id, status: "completed", result: { metadata: { exit } } };
}
const starts = () => calls.filter((call) => call.args[1] === "start");
const ends = () => calls.filter((call) => call.args[1] === "end");

// Loader-contract fixtures. These intentionally test the old all-exports
// function loop and V2's object shape, not a live OpenCode installation.
test("V1 exports exactly one callable initializer for legacy loaders", async () => {
	assert.deepEqual(Object.keys(legacy), ["default"]);
	const hooks = [];
	for (const initialize of Object.values(legacy)) {
		assert.equal(typeof initialize, "function");
		hooks.push(await initialize({ directory: "/project" }));
	}
	assert.equal(hooks.length, 1);
	assert.equal(typeof hooks[0]["shell.env"], "function");
	assert.equal(calls.length, 0);
});

test("V2 exports exactly one object definition with id and setup", () => {
	assert.deepEqual(Object.keys(v2), ["default"]);
	assert.equal(typeof v2.default, "object");
	assert.equal(v2.default.id, "atuin");
	assert.equal(typeof v2.default.setup, "function");
	assert.equal(v2.default.server, undefined);
});

test("V1 records only approved bash calls, keeping identical calls separate", async () => {
	const hooks = await legacy.default({ directory: "/project" });
	for (const id of ["A", "B", "denied"]) {
		await hooks["tool.execute.before"]({ tool: "bash", callID: id }, { args: { command: "same", description: id } });
	}
	assert.equal(calls.length, 0);
	await hooks["shell.env"]({ callID: "user-shell", cwd: "/manual" }, {});
	await hooks["shell.env"]({ callID: "B", cwd: "/B" }, {});
	await hooks["shell.env"]({ callID: "A", cwd: "/A" }, {});
	await hooks["tool.execute.after"]({ tool: "bash", callID: "A" }, { output: "", metadata: { exit: 7 } });
	await hooks["tool.execute.after"]({ tool: "bash", callID: "B" }, { output: "", metadata: { exit: 3 } });
	assert.deepEqual(starts().map((call) => [call.cwd, call.args.at(-3)]), [["/B", "B"], ["/A", "A"]]);
	assert.deepEqual(ends().map((call) => [call.cwd, call.args[2], call.args.at(-1)]), [["/A", "history-2", "7"], ["/B", "history-1", "3"]]);
});

test("V2 preserves argv, intent, resolved cwd, and exact completion ownership", async () => {
	const h = host();
	const dispose = await v2.default.setup(h.ctx);
	const command = "echo '$(touch /tmp/not-executed)'\nprintf '%s' \"$HOME\"";
	await h.emit("tool.execute.before", before("A", command, "explain A"));
	await h.emit("tool.execute.before", before("B", "echo B", "explain B"));
	assert.equal(calls.length, 0);
	await h.emit("shell.create.before", { command: "echo B", cwd: "/B" });
	await h.emit("shell.create.before", { command, cwd: "/A" });
	await h.emit("tool.execute.after", completed("A", 7));
	await h.emit("tool.execute.after", completed("B", 3));
	assert.equal(starts()[1].args.at(-1), command);
	assert.deepEqual(starts()[1].args.slice(-4), ["--intent", "explain A", "--", command]);
	assert.equal(starts()[1].options.shell, undefined);
	assert.deepEqual(ends().map((call) => [call.cwd, call.args[2], call.args.at(-1)]), [["/A", "history-2", "7"], ["/B", "history-1", "3"]]);
	await dispose();
});

for (const order of [["A", "B"], ["B", "A"]]) {
	test(`V2 skips indistinguishable commands when shell order is ${order.join(",")}`, async () => {
		const h = host();
		const dispose = await v2.default.setup(h.ctx);
		await h.emit("tool.execute.before", before("A", "same", "intent A"));
		await h.emit("tool.execute.before", before("B", "same", "intent B"));
		for (const id of order) {
			await h.emit("shell.create.before", { command: "same", cwd: `/${id}` });
			await h.emit("tool.execute.after", completed(id, id === "A" ? 3 : 7));
		}
		assert.equal(calls.length, 0);
		// Ambiguity is scoped to the overlapping calls, not forever.
		await h.emit("tool.execute.before", before("C", "same"));
		await h.emit("shell.create.before", { command: "same", cwd: "/C" });
		await h.emit("tool.execute.after", completed("C"));
		assert.equal(starts().length, 1);
		assert.equal(ends().length, 1);
		await dispose();
	});
}

test("V2 denied duplicate cannot make its survivor claimable", async () => {
	const h = host();
	const dispose = await v2.default.setup(h.ctx);
	await h.emit("tool.execute.before", before("A", "same"));
	await h.emit("tool.execute.before", before("B", "same"));
	await h.emit("tool.execute.after", { tool: "bash", id: "A", status: "error", error: "denied" });
	await h.emit("shell.create.before", { command: "same", cwd: "/B" });
	await h.emit("tool.execute.after", completed("B"));
	assert.equal(calls.length, 0);
	await dispose();
});

test("V2 late duplicate cannot steal an already running entry", async () => {
	const h = host();
	const dispose = await v2.default.setup(h.ctx);
	await h.emit("tool.execute.before", before("A", "same"));
	await h.emit("shell.create.before", { command: "same", cwd: "/A" });
	await h.emit("tool.execute.before", before("B", "same"));
	await h.emit("shell.create.before", { command: "same", cwd: "/B" });
	await h.emit("tool.execute.after", completed("B", 3));
	assert.equal(ends().length, 0);
	await h.emit("tool.execute.after", completed("A", 7));
	assert.equal(starts().length, 1);
	assert.deepEqual(ends()[0].args, ["history", "end", "history-1", "--exit", "7"]);
	assert.equal(ends()[0].cwd, "/A");
	await dispose();
});

test("V2 start and end use the same fallback directory", async () => {
	const h = host();
	const dispose = await v2.default.setup(h.ctx);
	await h.emit("tool.execute.before", before("A"));
	await h.emit("shell.create.before", { command: "echo test" });
	await h.emit("tool.execute.after", completed("A"));
	assert.deepEqual(calls.map((call) => call.cwd), ["/project", "/project"]);
	await dispose();
});

for (const failAt of [1, 2, 3]) {
	test(`V2 rolls back failure at registration ${failAt} and leaves callbacks inert`, async () => {
		const h = host({ failAt, disposeFailsAt: 2 });
		assert.equal(await v2.default.setup(h.ctx), undefined);
		assert.deepEqual(h.disposed, Array.from({ length: failAt - 1 }, (_, i) => failAt - 1 - i));
		await h.emit("tool.execute.before", before("A"));
		await h.emit("shell.create.before", { command: "echo test", cwd: "/A" });
		await h.emit("tool.execute.after", completed("A"));
		assert.equal(calls.length, 0);
	});
}

test("V2 callbacks stay inert until all registrations succeed", async () => {
	const h = host({ duringRegistration: async (callbacks) => {
		await callbacks.get("tool.execute.before")?.(before("A"));
		await callbacks.get("shell.create.before")?.({ command: "echo test", cwd: "/A" });
	} });
	const dispose = await v2.default.setup(h.ctx);
	assert.equal(calls.length, 0);
	await dispose();
});

test("V2 disposal is reverse-order, idempotent, and continues after a disposer rejects", async () => {
	const h = host({ disposeFailsAt: 2 });
	const dispose = await v2.default.setup(h.ctx);
	await dispose();
	await dispose();
	assert.deepEqual(h.disposed, [3, 2, 1]);
	await h.emit("tool.execute.before", before("A"));
	await h.emit("shell.create.before", { command: "echo test", cwd: "/A" });
	assert.equal(calls.length, 0);
});

test("V2 safely stops correlation at capacity while closing already-running entries", async () => {
	const h = host();
	const dispose = await v2.default.setup(h.ctx);
	await h.emit("tool.execute.before", before("running", "running"));
	await h.emit("shell.create.before", { command: "running", cwd: "/running" });
	for (let i = 0; i < 130; i++) await h.emit("tool.execute.before", before(`id-${i}`, `cmd-${i}`));
	await h.emit("tool.execute.before", before("replacement", "cmd-0"));
	await h.emit("shell.create.before", { command: "cmd-0", cwd: "/delayed" });
	await h.emit("tool.execute.after", completed("running", 9));
	assert.equal(starts().length, 1);
	assert.deepEqual(ends()[0].args, ["history", "end", "history-1", "--exit", "9"]);
	await dispose();
});

for (const ctx of [undefined, {}, { tool: {} }, { tool: { hook() {} }, shell: {} }]) {
	test(`V2 missing hook domains disable recording (${JSON.stringify(ctx)})`, async () => {
		assert.equal(await v2.default.setup(ctx), undefined);
		assert.equal(calls.length, 0);
	});
}

for (const mode of ["throw", "error", "nonzero", "empty"]) {
	test(`V1 and V2 tolerate Atuin ${mode} failures without ending a missing entry`, async () => {
		failure = mode;
		const hooks = await legacy.default({ directory: "/project" });
		await hooks["tool.execute.before"]({ tool: "bash", callID: "A" }, { args: { command: "test" } });
		await hooks["shell.env"]({ callID: "A", cwd: "/A" }, {});
		await hooks["tool.execute.after"]({ tool: "bash", callID: "A" }, { output: "", metadata: { exit: 0 } });
		const h = host();
		const dispose = await v2.default.setup(h.ctx);
		await h.emit("tool.execute.before", before("B"));
		await h.emit("shell.create.before", { command: "echo test", cwd: "/B" });
		await h.emit("tool.execute.after", completed("B"));
		assert.equal(ends().length, 0);
		await dispose();
	});
}

for (const [event, expected] of [
	[{ status: "completed", result: { metadata: { exit: 23 } } }, "23"],
	[{ status: "error", error: { message: "User aborted the command" } }, "130"],
	[{ status: "error", error: { message: "exceeding timeout 1000 ms" } }, "124"],
	[{ status: "error", error: { message: "failed" } }, "1"],
	[{ status: "completed", result: {} }, "0"],
]) {
	test(`V2 preserves completion/abort/timeout exit ${expected}`, async () => {
		const h = host();
		const dispose = await v2.default.setup(h.ctx);
		await h.emit("tool.execute.before", before("A"));
		await h.emit("shell.create.before", { command: "echo test", cwd: "/A" });
		await h.emit("tool.execute.after", { tool: "bash", id: "A", ...event });
		assert.equal(ends()[0].args.at(-1), expected);
		await dispose();
	});
}

test("V2 ignores non-bash proposals, denied calls, and shells with no proposal", async () => {
	const h = host();
	const dispose = await v2.default.setup(h.ctx);
	await h.emit("tool.execute.before", { ...before("other"), tool: "read" });
	await h.emit("shell.create.before", { command: "echo test", cwd: "/manual" });
	await h.emit("tool.execute.before", before("denied"));
	await h.emit("tool.execute.after", { tool: "bash", id: "denied", status: "error", error: "denied" });
	await h.emit("shell.create.before", { command: "echo test", cwd: "/manual" });
	assert.equal(calls.length, 0);
	await dispose();
});

test("V2 claims each proposal at most once and ignores repeated completion", async () => {
	const h = host();
	const dispose = await v2.default.setup(h.ctx);
	await h.emit("tool.execute.before", before("A"));
	await h.emit("shell.create.before", { command: "echo test", cwd: "/A" });
	await h.emit("shell.create.before", { command: "echo test", cwd: "/A" });
	await h.emit("tool.execute.after", completed("A"));
	await h.emit("tool.execute.after", completed("A"));
	assert.equal(starts().length, 1);
	assert.equal(ends().length, 1);
	await dispose();
});

test("V2 setup instances do not share proposals or lifecycle state", async () => {
	const a = host();
	const b = host();
	const disposeA = await v2.default.setup(a.ctx);
	const disposeB = await v2.default.setup(b.ctx);
	await a.emit("tool.execute.before", before("A", "same"));
	await b.emit("tool.execute.before", before("B", "same"));
	await disposeA();
	await b.emit("shell.create.before", { command: "same", cwd: "/B" });
	await b.emit("tool.execute.after", completed("B"));
	assert.equal(starts().length, 1);
	assert.equal(ends()[0].cwd, "/B");
	await disposeB();
});

test("V2 disposal during history start does not resurrect callback state", async () => {
	const h = host();
	const dispose = await v2.default.setup(h.ctx);
	await h.emit("tool.execute.before", before("A"));
	const start = h.emit("shell.create.before", { command: "echo test", cwd: "/A" });
	await dispose();
	await start;
	await h.emit("tool.execute.after", completed("A"));
	assert.equal(starts().length, 1);
	assert.equal(ends().length, 0);
});

test("V1 and V2 swallow history-end failures", async () => {
	const hooks = await legacy.default({ directory: "/project" });
	await hooks["tool.execute.before"]({ tool: "bash", callID: "A" }, { args: { command: "test" } });
	await hooks["shell.env"]({ callID: "A", cwd: "/A" }, {});
	failure = "throw";
	await hooks["tool.execute.after"]({ tool: "bash", callID: "A" }, { output: "", metadata: { exit: 0 } });
	failure = undefined;
	const h = host();
	const dispose = await v2.default.setup(h.ctx);
	await h.emit("tool.execute.before", before("B"));
	await h.emit("shell.create.before", { command: "echo test", cwd: "/B" });
	failure = "error";
	await h.emit("tool.execute.after", completed("B"));
	assert.equal(ends().length, 2);
	await dispose();
});
