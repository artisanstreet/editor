import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { appendFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const encoder = new TextEncoder();
const grandchild_path = fileURLToPath(new URL("./fake-grandchild.ts", import.meta.url));
type FixtureRecord = Record<string, any>;
type FrameOptions = { crlf?: boolean; split_at?: number };

const received: FixtureRecord[] = [];
let pending = "";
let active_turn_id: string | null = null;
let approval_resolved = false;
let questions_resolved = false;

const thread_start_params = new Set([
	"approvalPolicy",
	"approvalsReviewer",
	"baseInstructions",
	"config",
	"cwd",
	"developerInstructions",
	"ephemeral",
	"model",
	"modelProvider",
	"personality",
	"sandbox",
	"serviceName",
	"serviceTier",
	"sessionStartSource",
	"threadSource",
]);
const thread_resume_params = new Set([
	"approvalPolicy",
	"approvalsReviewer",
	"baseInstructions",
	"config",
	"cwd",
	"developerInstructions",
	"model",
	"modelProvider",
	"personality",
	"sandbox",
	"serviceTier",
	"threadId",
]);

/**
 * Records argv and piped stdin for the process-host launcher tests. The exec
 * transport itself is gone; this branch only proves the Windows cmd launcher
 * delivers metacharacter-bearing argv and stdin bytes verbatim.
 */
if (process.argv.includes("exec")) {
	if (process.env.FAKE_CODEX_EXEC_INVOCATION_FILE) {
		appendFileSync(
			process.env.FAKE_CODEX_EXEC_INVOCATION_FILE,
			`${JSON.stringify(process.argv.slice(2))}\n`,
		);
	}

	if (process.argv.includes("-") && process.env.FAKE_CODEX_EXEC_STDIN_FILE) {
		const chunks: Buffer[] = [];

		for await (const chunk of process.stdin) {
			chunks.push(chunk as Buffer);
		}

		writeFileSync(process.env.FAKE_CODEX_EXEC_STDIN_FILE, Buffer.concat(chunks));
	}

	process.exit(0);
}

if (process.argv.includes("--version")) {
	const version_scenarios = [process.env.FAKE_APP_SERVER_SCENARIO].filter(
		(scenario): scenario is string => scenario !== undefined,
	);

	if (version_scenarios.includes("version-fragmented")) {
		process.stdout.write("codex-cli 0.");
		await new Promise((resolve) => setImmediate(resolve));
		process.stdout.write("142.5\n");
		process.exit(0);
	}

	if (version_scenarios.includes("version-newer")) {
		process.stdout.write("codex-cli 0.146.0-alpha.3.1\n");
		process.exit(0);
	}

	if (version_scenarios.some((scenario) => scenario.startsWith("continuation"))) {
		process.stdout.write("codex-cli 0.145.0\n");
		process.exit(0);
	}

	if (version_scenarios.includes("version-older")) {
		process.stdout.write("codex-cli 0.142.4\n");
		process.exit(0);
	}

	if (version_scenarios.includes("version-stdout-overflow")) {
		await new Promise((resolve) => process.stdout.write("x".repeat(64 * 1_024 + 1), resolve));
		process.exit(0);
	}

	if (version_scenarios.includes("version-stderr-overflow")) {
		await new Promise((resolve) => process.stderr.write("x".repeat(64 * 1_024 + 1), resolve));
		process.exit(0);
	}

	process.stdout.write("codex-cli 0.142.5\n");
	process.exit(0);
}

if (process.env.FAKE_APP_SERVER_PID_FILE) {
	writeFileSync(process.env.FAKE_APP_SERVER_PID_FILE, String(process.pid));
}

function make_turn(id: string | null, status = "inProgress") {
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

function make_thread(id: string, turns: FixtureRecord[] = []) {
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

function make_thread_response(thread: FixtureRecord, resumed: boolean) {
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
			type: "workspace-write",
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

function write_frame(frame: FixtureRecord, options: FrameOptions = {}) {
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

function respond(id: unknown, result: unknown, options?: FrameOptions) {
	write_frame({ id, result }, options);
}

function validate_thread_params(request: FixtureRecord) {
	const allowed = request.method === "thread/start" ? thread_start_params : thread_resume_params;
	const unknown = Object.keys(request.params ?? {}).filter((key) => !allowed.has(key));

	if (unknown.length === 0) {
		return true;
	}

	write_frame({
		error: {
			code: -32602,
			message: `Unknown ${request.method} params: ${unknown.join(", ")}`,
		},
		id: request.id,
	});

	return false;
}

function handle_request(request: FixtureRecord) {
	received.push(request);

	if (request.method === "initialize") {
		if (process.env.FAKE_APP_SERVER_SCENARIO === "exec-fallback") {
			process.exit(23);
		}

		if (process.env.FAKE_APP_SERVER_SCENARIO === "stall-initialize") {
			return;
		}

		if (process.env.FAKE_APP_SERVER_SCENARIO === "startup-status-flood") {
			for (let index = 0; index < 1_200; index += 1) {
				write_frame({
					method:
						index % 3 === 0
							? "account/rateLimits/updated"
							: index % 3 === 1
								? "mcpServer/startupStatus/updated"
								: "remoteControl/status/changed",
					params: { index },
				});
			}
			write_frame({
				method: "turn/completed",
				params: { threadId: "thread-1", turn: make_turn("turn-1", "completed") },
			});
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
		if (!validate_thread_params(request)) {
			return;
		}

		if (process.env.FAKE_APP_SERVER_REQUEST_FILE) {
			appendFileSync(
				process.env.FAKE_APP_SERVER_REQUEST_FILE,
				`${JSON.stringify({ method: request.method, params: request.params })}\n`,
			);
		}

		if (process.env.FAKE_APP_SERVER_SCENARIO === "thread-start-failure") {
			write_frame({
				error: { code: -32010, message: "thread start failed ambiguously" },
				id: request.id,
			});

			return;
		}

		const thread_id =
			request.method === "thread/resume" ? request.params.threadId : "thread-started";
		const resumed = request.method === "thread/resume";
		const turns =
			resumed &&
			["resume-active", "resume-active-next-text", "steer-failure"].includes(
				process.env.FAKE_APP_SERVER_SCENARIO ?? "",
			)
				? [make_turn("turn-live")]
				: [];
		const thread = make_thread(thread_id, turns);

		if (turns.length > 0) {
			active_turn_id = String(turns[0]!.id);
		}

		write_frame({
			method: "thread/started",
			params: { thread },
		});
		respond(request.id, make_thread_response(thread, resumed));

		return;
	}

	if (request.method === "model/list") {
		if (process.env.FAKE_APP_SERVER_SCENARIO === "continuation-model-pagination") {
			respond(
				request.id,
				request.params.cursor === "models-page-2"
					? { data: [{ id: "gpt-later" }], nextCursor: null }
					: { data: [{ id: "gpt-5" }], nextCursor: "models-page-2" },
			);

			return;
		}
		respond(request.id, { data: [{ id: "gpt-5" }, { id: "gpt-5-mini" }] });

		return;
	}

	if (request.method === "thread/fork") {
		if (process.env.FAKE_APP_SERVER_SCENARIO === "continuation-fork-rejected") {
			write_frame({
				error: { code: -32601, message: "thread/fork unsupported" },
				id: request.id,
			});

			return;
		}
		if (
			request.params.threadId !== "thread-source" ||
			request.params.lastTurnId !== "turn-settled" ||
			request.params.cwd !== "C:\\workspace" ||
			request.params.ephemeral !== true ||
			request.params.approvalPolicy !== "never" ||
			request.params.sandbox !== "read-only" ||
			(process.env.FAKE_APP_SERVER_SCENARIO === "continuation" &&
				request.params.model !== "gpt-5")
		) {
			write_frame({
				error: { code: -32602, message: "invalid fork boundary" },
				id: request.id,
			});

			return;
		}
		respond(request.id, { thread: make_thread("thread-export") });

		return;
	}

	if (request.method === "turn/start") {
		if (process.env.FAKE_APP_SERVER_REQUEST_FILE) {
			appendFileSync(
				process.env.FAKE_APP_SERVER_REQUEST_FILE,
				`${JSON.stringify({ method: request.method, params: request.params })}\n`,
			);
		}

		active_turn_id = `turn-${received.length}`;

		if (process.env.FAKE_APP_SERVER_SCENARIO === "turn-start-failure") {
			write_frame({
				error: { code: -32011, message: "turn start failed ambiguously" },
				id: request.id,
			});

			return;
		}

		if (process.env.FAKE_APP_SERVER_SCENARIO !== "resume-active-next-text") {
			write_frame({
				method: "turn/started",
				params: {
					threadId: request.params.threadId,
					turn: make_turn(active_turn_id),
				},
			});
		}

		if (request.params.threadId === "thread-export") {
			const scenario = process.env.FAKE_APP_SERVER_SCENARIO;
			if (
				request.params.approvalPolicy !== "never" ||
				request.params.sandboxPolicy?.type !== "readOnly" ||
				request.params.outputSchema === undefined
			) {
				write_frame({
					error: { code: -32602, message: "unsafe export turn" },
					id: request.id,
				});

				return;
			}
			if (scenario === "continuation-pre-response") {
				write_frame({
					method: "item/completed",
					params: {
						item: {
							id: "export-message",
							memoryCitation: null,
							phase: "final",
							text: '{"summary":"handoff"}',
							type: "agentMessage",
						},
						threadId: "thread-export",
						turnId: active_turn_id,
					},
				});
				write_frame({
					method: "turn/completed",
					params: {
						threadId: "thread-export",
						turn: make_turn(active_turn_id, "completed"),
					},
				});
			}
			if (scenario === "continuation-generic-tool-pre-response") {
				write_frame({
					method: "item/started",
					params: {
						item: { id: "export-command", type: "commandExecution" },
						threadId: "thread-export",
						turnId: active_turn_id,
					},
				});
			}
			respond(request.id, { turn: make_turn(active_turn_id) });
			setTimeout(() => {
				if (scenario === "continuation-timeout") return;
				if (scenario === "continuation-pre-response") return;
				if (scenario === "continuation-generic-tool-pre-response") return;
				if (scenario === "continuation-server-request") {
					write_frame({
						id: "export-approval",
						method: "item/commandExecution/requestApproval",
						params: {},
					});

					return;
				}
				if (scenario === "continuation-tool") {
					write_frame({ method: "item/commandExecution/outputDelta", params: {} });

					return;
				}
				if (scenario === "continuation-foreign-thread") {
					write_frame({
						method: "item/completed",
						params: {
							item: { id: "foreign-command", type: "commandExecution" },
							threadId: "thread-foreign",
							turnId: active_turn_id,
						},
					});
					write_frame({
						method: "turn/completed",
						params: {
							threadId: "thread-foreign",
							turn: make_turn(active_turn_id, "completed"),
						},
					});
				}
				if (scenario === "continuation-failed" || scenario === "continuation-interrupted") {
					write_frame({
						method: "turn/completed",
						params: {
							threadId: "thread-export",
							turn: make_turn(
								active_turn_id,
								scenario === "continuation-failed" ? "failed" : "interrupted",
							),
						},
					});

					return;
				}
				if (scenario !== "continuation-missing-message") {
					write_frame({
						method: "item/completed",
						params: {
							item: {
								id: "export-message",
								memoryCitation: null,
								phase:
									scenario === "continuation-commentary-message"
										? "commentary"
										: "final",
								text: '{"summary":"handoff"}',
								type: "agentMessage",
							},
							threadId: "thread-export",
							turnId: active_turn_id,
						},
					});
					if (scenario === "continuation-duplicate-message")
						write_frame({
							method: "item/completed",
							params: {
								item: {
									id: "export-message-2",
									memoryCitation: null,
									phase: "final",
									text: '{"summary":"second"}',
									type: "agentMessage",
								},
								threadId: "thread-export",
								turnId: active_turn_id,
							},
						});
				}
				write_frame({
					method: "turn/completed",
					params: {
						threadId: "thread-export",
						turn: make_turn(active_turn_id, "completed"),
					},
				});
			}, 5);

			return;
		}

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
				for (let index = 0; index < 600; index += 1) {
					write_frame({ method: "fixture/event", params: { index } });
				}
				write_frame({
					method: "turn/completed",
					params: {
						threadId: request.params.threadId,
						turn: make_turn(active_turn_id, "completed"),
					},
				});
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

	if (request.method === "scenario/lossyNotificationFlood") {
		const count = request.params.count;

		for (let index = 0; index < count; index += 1) {
			write_frame({
				method: "item/agentMessage/delta",
				params: {
					delta: `delta-${index}`,
					itemId: "message-1",
					threadId: "thread-1",
					turnId: "turn-1",
				},
			});
		}

		respond(request.id, { count });

		return;
	}

	if (request.method === "scenario/lossyThenCriticalNotificationFlood") {
		const count = request.params.count;

		for (let index = 0; index < count; index += 1) {
			write_frame({
				method: "item/agentMessage/delta",
				params: {
					delta: `delta-${index}`,
					itemId: "message-1",
					threadId: "thread-1",
					turnId: "turn-1",
				},
			});
		}
		write_frame({
			method: "turn/completed",
			params: { threadId: "thread-1", turn: make_turn("turn-1", "completed") },
		});
		respond(request.id, { count });

		return;
	}

	if (request.method === "scenario/malformedThenNotificationFlood") {
		process.stdout.write("not json\n");
		const count = request.params.count;

		for (let index = 0; index < count; index += 1) {
			write_frame({ method: "test/notification", params: { index } });
		}

		respond(request.id, { count });

		return;
	}

	if (request.method === "scenario/additiveNotification") {
		write_frame({
			emittedAtMs: 42,
			method: "test/additive",
			params: { value: "preserved" },
		});
		respond(request.id, { ok: true });

		return;
	}

	if (request.method === "scenario/oversizedLine") {
		process.stdout.write(`{"type":"fixture.oversized","payload":"${"x".repeat(4_096)}`);

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
		write_frame({ extra: true, method: "", params: {} });
		write_frame({ id: null, method: "invalid/additive-id", params: {} });
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

	if (request.method === "scenario/inspectInitialize") {
		respond(request.id, {
			initialize: received.find((entry) => entry.method === "initialize"),
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
