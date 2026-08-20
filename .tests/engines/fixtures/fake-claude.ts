import { spawn } from "node:child_process";
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";

type FixtureEvent = Record<string, unknown>;

const is_record = (value: unknown): value is Record<string, unknown> =>
	typeof value === "object" && value !== null && !Array.isArray(value);

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
} else if (args.includes("auth") && args.includes("status")) {
	if (scenario === "auth-timeout") {
		setInterval(() => undefined, 1_000);
	} else if (scenario === "auth-bounds") {
		process.stdout.write("x".repeat(10_000));
		process.exit(0);
	} else if (scenario === "auth-unsupported") {
		process.stderr.write("unknown command\n");
		process.exit(1);
	} else if (scenario.endsWith("auth-heals")) {
		// Logged out on the first probe, healed afterwards; the invocation file
		// recorded this spawn above, so its auth count includes the current one.
		const auth_spawns = invocation_file
			? readFileSync(invocation_file, "utf8")
					.split("\n")
					.filter((line) => line.includes('"auth"')).length
			: 0;
		process.stdout.write(`${JSON.stringify({ loggedIn: auth_spawns >= 2 })}\n`);
		process.exit(0);
	} else {
		process.stdout.write(`${JSON.stringify({ loggedIn: scenario !== "auth-unauth" })}\n`);
		process.exit(0);
	}
} else if (args.includes("/usage")) {
	/**
	 * The real CLI takes its prompt from stdin here and waits out a grace period
	 * before proceeding without it, so a caller that never sends EOF pays that
	 * wait on every read. Exiting only once stdin ends keeps that cost visible to
	 * anything that forgets to close it.
	 */
	process.stdin.resume();
	process.stdin.on("end", () => {
		process.stdout.write(
			`${JSON.stringify({
				result:
					"Current session: 12% used · resets Aug 16, 5:50am (Europe/Oslo)\n" +
					"Current week (all models): 28% used · resets Aug 19, 12am (Europe/Oslo)",
			})}\n`,
		);
		process.exit(0);
	});
} else {
	const decoder = new TextDecoder();
	let input = "";
	let interactive_buffer = "";
	const interactive_messages: Array<FixtureEvent> = [];
	let interactive_started = false;
	let interactive_approved = false;
	let interactive_finished = false;
	let immediate_finished = false;
	const write = (event: FixtureEvent) => process.stdout.write(`${JSON.stringify(event)}\n`);
	const interactive_session = () => {
		const session_index = args.indexOf("--session-id");
		const resume_index = args.indexOf("--resume");
		return session_index >= 0 ? args[session_index + 1] : args[resume_index + 1];
	};
	const interactive = () => {
		const session_id = interactive_session();
		if (!interactive_started) {
			interactive_started = true;
			write({ type: "system", subtype: "init", session_id, model: "fake", tools: [] });
			write({
				type: "control_request",
				request_id: "permission-1",
				request: {
					subtype: "can_use_tool",
					tool_name: "Bash",
					input: { command: "echo safe" },
					title: "Approve fake Bash",
				},
			});
			return;
		}
		const response = interactive_messages.find((line) => line.type === "control_response");
		if (response && !interactive_approved) {
			const envelope = response.response;
			const result = is_record(envelope) ? envelope.response : undefined;
			const expected_behavior = scenario === "interactive-deny" ? "deny" : "allow";
			if (
				!is_record(envelope) ||
				envelope.subtype !== "success" ||
				envelope.request_id !== "permission-1" ||
				!is_record(result) ||
				result.behavior !== expected_behavior
			) {
				write({
					type: "result",
					subtype: "error_during_execution",
					is_error: true,
					session_id,
					errors: ["invalid control_response envelope"],
				});
				interactive_finished = true;
				setTimeout(() => process.exit(0), 10);
				return;
			}
			interactive_approved = true;
			return;
		}
		if (
			!interactive_finished &&
			interactive_approved &&
			interactive_messages.filter((line) => line.type === "user").length >= 2
		) {
			interactive_finished = true;
			write({
				type: "assistant",
				session_id,
				message: {
					content: [
						{
							type: "tool_use",
							id: "task-tool",
							name: "Task",
							input: { prompt: "child" },
						},
					],
				},
			});
			write({
				type: "system",
				subtype: "task_started",
				session_id,
				task_id: "child-task",
				tool_use_id: "task-tool",
				subagent_type: "general-purpose",
				description: "child",
			});
			write({
				type: "assistant",
				session_id: "child-task",
				parent_tool_use_id: "task-tool",
				message: { content: [{ type: "text", text: "child reply" }] },
			});
			write({
				type: "result",
				subtype: "success",
				session_id,
				usage: { input_tokens: 1, output_tokens: 1 },
			});
			setTimeout(() => process.exit(0), 10);
		}
	};
	/**
	 * Claude asks its questions over the permission channel and stays blocked
	 * until every question in the request has an answer, which it collects from
	 * the allow response's `updatedInput.answers`, keyed by question text.
	 */
	const question_one = "Which library should we use?";
	const question_two = "Which deployment targets?";
	let question_started = false;
	let question_finished = false;
	const question = () => {
		const session_id = interactive_session();
		if (!question_started) {
			question_started = true;
			write({ type: "system", subtype: "init", session_id, model: "fake", tools: [] });
			write({
				type: "control_request",
				request_id: "question-1",
				request: {
					subtype: "can_use_tool",
					tool_name: "AskUserQuestion",
					input: {
						questions: [
							{
								header: "Library",
								multiSelect: false,
								options: [
									{ label: "Effect", description: "Already in the workspace" },
									{ label: "RxJS", description: "One more dependency" },
								],
								question: question_one,
							},
							{
								header: "Targets",
								multiSelect: true,
								options: [{ label: "Desktop" }, { label: "Web" }],
								question: question_two,
							},
						],
					},
				},
			});
			return;
		}
		if (question_finished) return;
		const response = interactive_messages.find((line) => line.type === "control_response");
		if (!response) return;
		question_finished = true;
		const envelope = response.response;
		const result = is_record(envelope) ? envelope.response : undefined;
		const updated = is_record(result) ? result.updatedInput : undefined;
		const answers = is_record(updated) ? updated.answers : undefined;
		const valid =
			is_record(envelope) &&
			envelope.subtype === "success" &&
			envelope.request_id === "question-1" &&
			is_record(result) &&
			result.behavior === "allow" &&
			is_record(answers) &&
			answers[question_one] === "Effect" &&
			answers[question_two] === "Desktop, Web";
		write(
			valid
				? {
						type: "result",
						subtype: "success",
						session_id,
						usage: { input_tokens: 1, output_tokens: 1 },
					}
				: {
						type: "result",
						subtype: "error_during_execution",
						is_error: true,
						session_id,
						errors: ["invalid question control_response"],
					},
		);
		setTimeout(() => process.exit(0), 10);
	};
	const immediate = () => {
		if (immediate_finished) return;
		immediate_finished = true;
		const session_id = interactive_session();
		const scenario_suffix = scenario.slice("immediate-".length);
		if (scenario_suffix === "inactivity") {
			setInterval(() => undefined, 1_000);
			return;
		}
		if (scenario_suffix === "cancel-tree") {
			if (grandchild_file === undefined)
				throw new Error("cancel-tree requires a grandchild pid file");
			const child = spawn(process.execPath, ["-e", "setInterval(() => undefined, 1000)"], {
				stdio: "ignore",
				windowsHide: true,
			});
			writeFileSync(
				grandchild_file,
				JSON.stringify({ grandchild_pid: child.pid, provider_pid: process.pid }),
			);
			write({ type: "system", subtype: "init", session_id, model: "fake", tools: [] });
			setInterval(() => undefined, 1_000);
			return;
		}
		if (scenario_suffix === "stderr-bounds") process.stderr.write("x".repeat(10_000));
		if (scenario_suffix === "malformed") process.stdout.write("not-json\n");
		if (scenario_suffix === "oversized")
			process.stdout.write(
				`${JSON.stringify({ type: "future", payload: "x".repeat(5_000) })}\n`,
			);
		if (scenario_suffix !== "missing-init")
			write({
				type: "system",
				subtype: "init",
				session_id: scenario_suffix === "mismatch" ? "wrong-session" : session_id,
				model: "fake",
				tools: [],
			});
		if (scenario_suffix === "semantic-failure")
			write({
				type: "result",
				subtype: "error_during_execution",
				is_error: true,
				session_id,
				errors: ["sanitized failure"],
			});
		else if (scenario_suffix !== "missing-result")
			write({
				type: "result",
				subtype: "success",
				session_id,
				usage: { input_tokens: 1, output_tokens: 1 },
			});
		setTimeout(() => process.exit(scenario_suffix === "nonzero" ? 7 : 0), 10);
	};
	if (scenario === "immediate-empty-resume") setImmediate(immediate);
	process.stdin.on("data", (chunk) => {
		const decoded = typeof chunk === "string" ? chunk : decoder.decode(chunk, { stream: true });
		input += decoded;
		if (invocation_file)
			appendFileSync(invocation_file, `${JSON.stringify({ stdin_chunk: decoded })}\n`);
		if (
			scenario === "interactive" ||
			scenario === "interactive-deny" ||
			scenario === "interactive-question"
		) {
			interactive_buffer += decoded;
			for (;;) {
				const line_end = interactive_buffer.indexOf("\n");
				if (line_end < 0) break;
				const line = interactive_buffer.slice(0, line_end);
				interactive_buffer = interactive_buffer.slice(line_end + 1);
				if (line) interactive_messages.push(JSON.parse(line) as FixtureEvent);
			}
			if (scenario === "interactive-question") question();
			else interactive();
		}
		if (scenario.startsWith("immediate-")) immediate();
	});
	process.stdin.on("end", () => {
		if (
			scenario === "interactive" ||
			scenario === "interactive-deny" ||
			scenario === "interactive-question"
		) {
			if (invocation_file)
				appendFileSync(invocation_file, `${JSON.stringify({ stdin: input })}\n`);
			process.exit(0);
			return;
		}
		if (scenario.startsWith("immediate-")) {
			if (invocation_file)
				appendFileSync(invocation_file, `${JSON.stringify({ stdin: input })}\n`);
			return;
		}
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
