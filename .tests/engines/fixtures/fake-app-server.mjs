import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { appendFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const encoder = new TextEncoder();
const grandchild_path = fileURLToPath(new URL("./fake-grandchild.mjs", import.meta.url));
const received = [];
let pending = "";

if (process.env.FAKE_APP_SERVER_PID_FILE) {
	writeFileSync(process.env.FAKE_APP_SERVER_PID_FILE, String(process.pid));
}

function write_frame(frame, options = {}) {
	const text = `${JSON.stringify(frame)}${options.crlf ? "\r\n" : "\n"}`;
	const bytes = encoder.encode(text);
	const split_at = options.split_at;

	if (split_at === undefined) {
		process.stdout.write(bytes);

		return;
	}

	process.stdout.write(bytes.subarray(0, split_at));
	setTimeout(() => process.stdout.write(bytes.subarray(split_at)), 2);
}

function respond(id, result, options) {
	write_frame({ id, result }, options);
}

function handle_request(request) {
	received.push(request);

	if (request.method === "initialize") {
		const frame = {
			id: request.id,
			result: {
				codexHome: "C:\\fake-codex",
				platformFamily: "windows",
				platformOs: "win32",
				userAgent: "fake-codex snowman: \u2603",
			},
		};
		const bytes = encoder.encode(JSON.stringify(frame));
		const snowman_start = bytes.indexOf(0xe2);
		const split_at = snowman_start + 1;

		assert.deepEqual([...bytes.subarray(snowman_start, snowman_start + 3)], [0xe2, 0x98, 0x83]);
		assert.ok(split_at > snowman_start && split_at < snowman_start + 3);
		write_frame(frame, { split_at });

		return;
	}

	if (request.method === "scenario/frames") {
		process.stdout.write("not json\n");
		process.stdout.write(
			`${JSON.stringify({ method: "thread/started", params: { threadId: "thread-1" } })}\r\n${JSON.stringify({ id: "approval-1", method: "item/tool/requestUserInput", params: { question: "Continue?" } })}\n`,
		);
		respond(request.id, { ok: true });

		return;
	}

	if (request.method === "scenario/notificationFlood") {
		const count = request.params.count;

		for (let index = 0; index < count; index += 1) {
			write_frame({ method: "test/notification", params: { index } });
		}

		respond(request.id, { count });

		return;
	}

	if (request.method === "scenario/invalidEnvelopes") {
		write_frame({
			error: { code: -1, message: "both" },
			id: request.id,
			result: { invalid: true },
		});
		write_frame({ id: 1.5, result: {} });
		write_frame({ error: { code: 1.5, message: "fractional code" }, id: request.id });
		write_frame({ jsonrpc: "1.0", method: "invalid/jsonrpc" });
		write_frame({ extra: true, method: "invalid/excess", params: {} });
		respond(request.id, { ok: true });

		return;
	}

	if (request.method === "scenario/respondThenExit") {
		process.stdin.destroy();
		process.stdout.end(`${JSON.stringify({ id: request.id, result: { final: true } })}\n`, () =>
			process.exit(0),
		);

		return;
	}

	if (request.method === "scenario/stderr") {
		for (let index = 0; index < 512; index += 1) {
			process.stderr.write(`diagnostic-${index}\n`);
		}

		respond(request.id, { ok: true });

		return;
	}

	if (request.method === "scenario/crash") {
		setTimeout(() => process.exit(23), 5);

		return;
	}

	if (request.method === "scenario/late") {
		setTimeout(() => respond(request.id, { late: true }), request.params.delay_ms ?? 60);

		return;
	}

	if (request.method === "scenario/inspect") {
		respond(request.id, {
			received: received.map((entry) => ({ id: entry.id, method: entry.method })),
		});

		return;
	}

	if (request.method === "scenario/pid") {
		respond(request.id, { pid: process.pid });

		return;
	}

	if (request.method === "scenario/processTree") {
		const grandchild = spawn(process.execPath, [grandchild_path], {
			stdio: "ignore",
			windowsHide: true,
		});

		respond(request.id, { grandchild_pid: grandchild.pid, pid: process.pid });

		return;
	}

	if (request.method === "slow") {
		setTimeout(() => respond(request.id, { value: request.params?.value }), 40);

		return;
	}

	if (request.method === "fast") {
		setTimeout(() => respond(request.id, { value: request.params?.value }), 5);

		return;
	}

	if (request.id === "approval-1") {
		if (process.env.FAKE_APP_SERVER_RESPONSE_FILE) {
			appendFileSync(
				process.env.FAKE_APP_SERVER_RESPONSE_FILE,
				`${JSON.stringify(request)}\n`,
			);
		}

		return;
	}

	respond(request.id, { echo: request.params });
}

process.stdin.on("data", (chunk) => {
	pending += chunk.toString("utf8");

	while (true) {
		const newline = pending.indexOf("\n");

		if (newline === -1) {
			return;
		}

		const line = pending.slice(0, newline).replace(/\r$/, "");

		pending = pending.slice(newline + 1);

		if (!line) {
			continue;
		}

		handle_request(JSON.parse(line));
	}
});
