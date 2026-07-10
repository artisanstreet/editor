import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { appendFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const encoder = new TextEncoder();
const grandchild_path = fileURLToPath(new URL("./fake-grandchild.mjs", import.meta.url));
const received = [];
let pending = "";
let active_turn_id = null;
let approval_resolved = false;
let questions_resolved = false;

if (process.argv.includes("--version")) {
	process.stdout.write("codex-cli 0.142.5\n");
	process.exit(0);
}

if (process.env.FAKE_APP_SERVER_PID_FILE) {
	writeFileSync(process.env.FAKE_APP_SERVER_PID_FILE, String(process.pid));
}

function make_turn(id, status = "inProgress") {
	return {
		completedAt: status === "inProgress" ? null : 2,
		durationMs: status === "inProgress" ? null : 1_000,
		error: null,
		id,
		items: [],
		itemsView: "full",
		startedAt: 1,
		status,
	};
}

function make_thread(id, turns = []) {
	return {
		agentNickname: null,
		agentRole: null,
		cliVersion: "0.142.5",
		createdAt: 1,
		cwd: "C:\\workspace",
		ephemeral: false,
		forkedFromId: null,
		gitInfo: null,
		id,
		modelProvider: "openai",
		name: null,
		parentThreadId: null,
		path: null,
		preview: "Fixture thread",
		recencyAt: 1,
		sessionId: "fixture-session",
		source: "appServer",
		status: { activeFlags: [], type: "active" },
		threadSource: null,
		turns,
		updatedAt: 1,
	};
}

function make_thread_response(thread, resumed) {
	return {
		activePermissionProfile: { extends: null, id: ":workspace" },
		approvalPolicy: "on-request",
		approvalsReviewer: "user",
		cwd: thread.cwd,
		instructionSources: [],
		...(resumed ? { initialTurnsPage: null } : {}),
		model: "gpt-5",
		modelProvider: "openai",
		multiAgentMode: "explicitRequestOnly",
		reasoningEffort: "high",
		runtimeWorkspaceRoots: [thread.cwd],
		sandbox: {
			excludeSlashTmp: false,
			excludeTmpdirEnvVar: false,
			networkAccess: false,
			type: "workspaceWrite",
			writableRoots: [thread.cwd],
		},
		serviceTier: null,
		thread,
	};
}

function complete_request_turn() {
	if (!approval_resolved || !questions_resolved) {
		return;
	}

	write_frame({
		method: "turn/completed",
		params: {
			threadId: "thread-resumed",
			turn: make_turn(active_turn_id, "completed"),
		},
	});
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
		if (process.env.FAKE_APP_SERVER_SCENARIO === "stall-initialize") {
			return;
		}

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

	if (request.method === "account/read") {
		if (process.env.FAKE_APP_SERVER_SCENARIO === "bedrock") {
			respond(request.id, {
				account: { credentialSource: "codexManaged", type: "amazonBedrock" },
				requiresOpenaiAuth: false,
			});

			return;
		}

		respond(request.id, {
			account: { email: "fake@example.com", planType: "plus", type: "chatgpt" },
			requiresOpenaiAuth: false,
		});

		return;
	}

	if (request.method === "thread/start" || request.method === "thread/resume") {
		const thread_id =
			request.method === "thread/resume" ? request.params.threadId : "thread-started";
		const resumed = request.method === "thread/resume";
		const turns =
			resumed &&
			["resume-active", "steer-failure"].includes(process.env.FAKE_APP_SERVER_SCENARIO)
				? [make_turn("turn-live")]
				: [];
		const thread = make_thread(thread_id, turns);

		if (turns.length > 0) {
			active_turn_id = turns[0].id;
		}

		write_frame({
			method: "thread/started",
			params: { thread },
		});
		respond(request.id, make_thread_response(thread, resumed));

		return;
	}

	if (request.method === "turn/start") {
		active_turn_id = `turn-${received.length}`;
		write_frame({
			method: "turn/started",
			params: {
				threadId: request.params.threadId,
				turn: make_turn(active_turn_id),
			},
		});
		respond(request.id, { turn: make_turn(active_turn_id) });

		if (process.env.FAKE_APP_SERVER_SCENARIO === "stale-turn") {
			const stale_turn_id = active_turn_id;

			setTimeout(() => {
				active_turn_id = "turn-newer";
				write_frame({
					method: "turn/started",
					params: {
						threadId: request.params.threadId,
						turn: make_turn(active_turn_id),
					},
				});
				write_frame({
					method: "turn/completed",
					params: {
						threadId: request.params.threadId,
						turn: make_turn(stale_turn_id, "completed"),
					},
				});
			}, 5);
		}

		if (process.env.FAKE_APP_SERVER_SCENARIO === "complete") {
			setTimeout(() => {
				write_frame({
					method: "item/agentMessage/delta",
					params: {
						delta: "hello",
						itemId: "message-1",
						threadId: request.params.threadId,
						turnId: active_turn_id,
					},
				});
				write_frame({
					method: "turn/completed",
					params: {
						threadId: request.params.threadId,
						turn: make_turn(active_turn_id, "completed"),
					},
				});
			}, 5);
		}

		if (process.env.FAKE_APP_SERVER_SCENARIO === "requests") {
			setTimeout(() => {
				write_frame({
					id: "approval-request",
					method: "item/commandExecution/requestApproval",
					params: {
						availableDecisions: ["accept", "decline"],
						command: "pnpm test",
						commandActions: [],
						cwd: "C:\\workspace",
						environmentId: null,
						itemId: "command-1",
						reason: "Run the test suite",
						startedAtMs: 10,
						threadId: request.params.threadId,
						turnId: active_turn_id,
					},
				});
				write_frame({
					id: "question-request",
					method: "item/tool/requestUserInput",
					params: {
						autoResolutionMs: null,
						itemId: "question-tool",
						questions: [
							{
								header: "One",
								id: "question-1",
								isOther: false,
								isSecret: false,
								options: [{ description: "Use one", label: "One" }],
								question: "First?",
							},
							{
								header: "Two",
								id: "question-2",
								isOther: true,
								isSecret: false,
								options: null,
								question: "Second?",
							},
						],
						threadId: request.params.threadId,
						turnId: active_turn_id,
					},
				});
			}, 5);
		}

		if (process.env.FAKE_APP_SERVER_SCENARIO === "run-crash") {
			setTimeout(() => process.exit(23), 5);
		}

		if (process.env.FAKE_APP_SERVER_SCENARIO === "run-diagnostics") {
			setTimeout(() => process.stderr.write("first diagnostic\n"), 2);
			setTimeout(() => process.stderr.write("second diagnostic\n"), 5);
			setTimeout(
				() =>
					write_frame({
						method: "turn/completed",
						params: {
							threadId: request.params.threadId,
							turn: make_turn(active_turn_id, "completed"),
						},
					}),
				10,
			);
		}

		if (process.env.FAKE_APP_SERVER_SCENARIO === "event-flood") {
			setTimeout(() => {
				for (let index = 0; index < 32; index += 1) {
					write_frame({ method: "fixture/event", params: { index } });
				}
			}, 5);
		}

		return;
	}

	if (request.method === "turn/steer") {
		if (process.env.FAKE_APP_SERVER_SCENARIO === "steer-failure") {
			write_frame({
				method: "fixture/steerReceived",
				params: { commandIndex: received.length },
			});
			write_frame({ error: { code: -32001, message: "steer failed" }, id: request.id });

			return;
		}

		if (request.params.expectedTurnId !== active_turn_id) {
			write_frame({ error: { code: -32000, message: "wrong turn" }, id: request.id });

			return;
		}

		respond(request.id, { turn: { id: active_turn_id } });

		return;
	}

	if (request.method === "turn/interrupt") {
		write_frame({
			method: "turn/completed",
			params: {
				threadId: request.params.threadId,
				turn: make_turn(request.params.turnId, "interrupted"),
			},
		});
		respond(request.id, { turn: make_turn(request.params.turnId, "interrupted") });

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

	if (request.id === "approval-request") {
		assert.deepEqual(request.result, { decision: "accept" });
		approval_resolved = true;
		complete_request_turn();

		return;
	}

	if (request.id === "question-request") {
		assert.deepEqual(request.result, {
			answers: {
				"question-1": { answers: ["one"] },
				"question-2": { answers: ["two"] },
			},
		});
		questions_resolved = true;
		complete_request_turn();

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
