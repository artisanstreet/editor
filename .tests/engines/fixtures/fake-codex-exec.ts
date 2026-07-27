import { spawn } from "node:child_process";
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const transcript_path = fileURLToPath(
	new URL("./transcripts/codex-exec-jsonl.jsonl", import.meta.url),
);
const grandchild_path = fileURLToPath(new URL("./fake-grandchild.ts", import.meta.url));

function record_invocation() {
	if (!process.env.FAKE_CODEX_EXEC_INVOCATION_FILE) {
		return;
	}

	appendFileSync(
		process.env.FAKE_CODEX_EXEC_INVOCATION_FILE,
		`${JSON.stringify(process.argv.slice(2))}\n`,
	);
}

async function record_stdin() {
	if (!process.env.FAKE_CODEX_EXEC_STDIN_FILE) {
		return;
	}

	const chunks = [];

	for await (const chunk of process.stdin) {
		chunks.push(chunk);
	}

	writeFileSync(process.env.FAKE_CODEX_EXEC_STDIN_FILE, Buffer.concat(chunks));
}

function write_fragmented(bytes: Buffer) {
	const splits = [7, 31, 79, 151].filter((offset) => offset < bytes.length);
	let start = 0;

	for (const end of [...splits, bytes.length]) {
		process.stdout.write(bytes.subarray(start, end));
		start = end;
	}
}

function write_event(event: object) {
	process.stdout.write(`${JSON.stringify(event)}\n`);
}

function flush_stdout() {
	return new Promise((resolve) => process.stdout.write("", resolve));
}

function hang_with_optional_grandchild() {
	if (process.env.FAKE_CODEX_EXEC_GRANDCHILD_PID_FILE) {
		const grandchild = spawn(process.execPath, [grandchild_path], {
			stdio: "ignore",
			windowsHide: true,
		});

		writeFileSync(process.env.FAKE_CODEX_EXEC_GRANDCHILD_PID_FILE, String(grandchild.pid));
	}

	setInterval(() => undefined, 1_000);
}

export async function run_fake_codex_exec() {
	record_invocation();

	if (process.argv.includes("-")) {
		await record_stdin();
	}

	if (process.env.FAKE_CODEX_EXEC_PID_FILE) {
		writeFileSync(process.env.FAKE_CODEX_EXEC_PID_FILE, String(process.pid));
	}

	const scenario = process.env.FAKE_CODEX_EXEC_SCENARIO ?? "transcript";

	if (scenario === "transcript") {
		process.stderr.write("sanitized exec diagnostic\n");
		write_fragmented(readFileSync(transcript_path));

		return;
	}

	if (scenario === "malformed") {
		process.stdout.write("not json\n");
		write_event({ type: "thread.started", thread_id: "thread-malformed" });
		write_event({ type: "turn.started" });
		write_event({ type: "turn.completed", usage: {} });

		return;
	}

	if (scenario === "oversized") {
		write_event({ payload: "x".repeat(4_096), type: "future.oversized" });

		return;
	}

	if (scenario === "stdout-overflow") {
		for (let index = 0; index < 128; index += 1) {
			write_event({ index, type: "future.event" });
		}

		return;
	}

	if (scenario === "stderr-overflow") {
		process.stderr.write("x".repeat(4_096));
		hang_with_optional_grandchild();

		return;
	}

	if (scenario === "nonzero") {
		write_event({ message: "provider rejected the run", type: "error" });
		process.stderr.write("nonzero exec failure\n");
		process.exit(17);
	}

	if (scenario === "turn-failed-exit-zero") {
		write_event({ type: "thread.started", thread_id: "thread-turn-failed" });
		write_event({ type: "turn.started" });
		write_event({ type: "turn.failed" });
		await flush_stdout();

		return;
	}

	if (scenario === "error-exit-zero") {
		write_event({ type: "thread.started", thread_id: "thread-error" });
		write_event({ type: "turn.started" });
		write_event({ message: "provider reported a fatal error", type: "error" });
		await flush_stdout();

		return;
	}

	if (scenario === "hang" || scenario === "hang-ignore-term") {
		if (scenario === "hang-ignore-term" && process.platform !== "win32") {
			process.on("SIGTERM", () => undefined);
		}

		write_event({ type: "thread.started", thread_id: "thread-hang" });
		write_event({ type: "turn.started" });
		hang_with_optional_grandchild();
		write_event({ type: "fixture.ready" });
	}
}
