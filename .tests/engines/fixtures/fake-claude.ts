import { spawn } from "node:child_process";
import { appendFileSync, writeFileSync } from "node:fs";

type FixtureEvent = Record<string, unknown>;

const args = process.argv.slice(2);
const scenario = process.env.FAKE_CLAUDE_SCENARIO ?? "transcript";
const invocation_file = process.env.FAKE_CLAUDE_INVOCATION_FILE;
const grandchild_file = process.env.FAKE_CLAUDE_GRANDCHILD_PID_FILE;
const version = process.env.FAKE_CLAUDE_VERSION ?? "2.1.220";

if (invocation_file) appendFileSync(invocation_file, `${JSON.stringify({ args })}\n`);

if (args.includes("--version")) {
	if (scenario === "version-timeout") {
		setInterval(() => undefined, 1_000);
	} else if (scenario === "version-nonzero") {
		process.stderr.write("version unavailable\n");
		process.exit(9);
	} else if (scenario === "version-bounds") {
		process.stdout.write("x".repeat(10_000));
		process.exit(0);
	} else {
		process.stdout.write(`${version}\n`);
		process.exit(0);
	}
} else if (args[0] === "auth" && args[1] === "status") {
	if (scenario === "auth-timeout") {
		setInterval(() => undefined, 1_000);
	} else if (scenario === "auth-bounds") {
		process.stdout.write("x".repeat(10_000));
		process.exit(0);
	} else if (scenario === "auth-unsupported") {
		process.stderr.write("unknown command\n");
		process.exit(1);
	} else {
		process.stdout.write(`${JSON.stringify({ loggedIn: scenario !== "auth-unauth" })}\n`);
		process.exit(0);
	}
} else {
	const decoder = new TextDecoder();
	let input = "";
	process.stdin.on("data", (chunk) => {
		input += typeof chunk === "string" ? chunk : decoder.decode(chunk, { stream: true });
	});
	process.stdin.on("end", () => {
		if (invocation_file)
			appendFileSync(invocation_file, `${JSON.stringify({ stdin: input })}\n`);
		if (scenario === "timeout") {
			setInterval(() => undefined, 1_000);
			return;
		}
		if (grandchild_file) {
			const child = spawn(process.execPath, ["-e", "setInterval(() => undefined, 1000)"], {
				stdio: "ignore",
				windowsHide: true,
			});
			writeFileSync(grandchild_file, String(child.pid));
		}
		const session_index = args.indexOf("--session-id");
		const resume_index = args.indexOf("--resume");
		const session_id = session_index >= 0 ? args[session_index + 1] : args[resume_index + 1];
		const events: Array<FixtureEvent> =
			scenario === "semantic-failure"
				? [
						{
							type: "system",
							subtype: "init",
							session_id,
							model: "fake",
							tools: [],
							permissionMode: "default",
						},
						{
							type: "result",
							subtype: "error_during_execution",
							is_error: true,
							session_id,
							errors: ["sanitized failure"],
						},
					]
				: [
						{
							type: "system",
							subtype: "init",
							session_id,
							model: "fake",
							tools: ["Bash", "Edit", "WebSearch"],
							permissionMode: "default",
						},
						...(scenario === "post-compact"
							? [
									{
										type: "system",
										subtype: "compact_boundary",
										uuid: "boundary-1",
										compactMetadata: { trigger: "auto" },
									} satisfies FixtureEvent,
								]
							: []),
						{
							type: "stream_event",
							session_id,
							event: {
								type: "content_block_delta",
								delta: { type: "text_delta", text: "café " },
							},
						},
						{
							type: "assistant",
							session_id,
							message: {
								content: [
									{ type: "text", text: "done" },
									{ type: "thinking", thinking: "private" },
									{
										type: "tool_use",
										id: "tool-1",
										name: "Bash",
										input: { command: "printf ok" },
									},
								],
							},
						},
						{
							type: "user",
							session_id,
							message: {
								content: [
									{ type: "tool_result", tool_use_id: "tool-1", content: "ok" },
								],
							},
						},
						{
							type: "result",
							subtype: "success",
							session_id,
							usage: { input_tokens: 4, output_tokens: 5 },
						},
					];
		if (scenario === "missing-init") events.shift();
		if (scenario === "mismatch") events[0]!.session_id = "wrong-session";
		if (scenario === "flood")
			for (let index = 0; index < 20; index += 1)
				events.push({
					type: "stream_event",
					session_id,
					event: {
						type: "content_block_delta",
						delta: { type: "text_delta", text: String(index) },
					},
				});
		if (scenario === "malformed") process.stdout.write("not-json\n");
		if (scenario === "oversized")
			process.stdout.write(
				`${JSON.stringify({ type: "future", payload: "x".repeat(5_000) })}\n`,
			);
		if (scenario === "stderr")
			process.stderr.write(Buffer.from("é diagnostic\n").subarray(0, 2));
		if (scenario === "conformance") events.pop();
		for (const event of events) {
			const bytes = Buffer.from(`${JSON.stringify(event)}\n`);
			for (let index = 0; index < bytes.length; index += 3)
				process.stdout.write(bytes.subarray(index, index + 3));
		}
		if (scenario === "conformance") {
			setInterval(() => undefined, 1_000);
			return;
		}
		if (scenario === "nonzero") process.exit(7);
		process.exit(0);
	});
}
