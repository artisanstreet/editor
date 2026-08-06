import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it } from "vitest";
import { Cause, Effect, Exit, Fiber, Layer, Stream } from "effect";

import {
	ClaudeEngine,
	ClaudeEngineDescriptor,
	EngineProcessFactory,
	EngineRegistry,
	EngineProcessFactoryLive,
	make_claude_engine_layer,
	make_engine_registry_layer,
	type EngineObservation,
} from "@artisan/engines";
import type { SDKMessage } from "@anthropic-ai/claude-agent-sdk";

import {
	make_fake_claude_query,
	make_unstartable_claude_query,
	type FakeClaudeQuery,
	type FakeClaudeSession,
} from "./fixtures/fake-claude-query";
import {
	claude_assistant_text_frame,
	claude_init_frame,
	claude_text_delta_frame,
} from "./fixtures/claude-stream-frames";

const executable = fileURLToPath(new URL("./fixtures/fake-claude.ts", import.meta.url));
const original_scenario = process.env.FAKE_CLAUDE_SCENARIO;
const original_version = process.env.FAKE_CLAUDE_VERSION;

afterEach(() => {
	if (original_scenario === undefined) delete process.env.FAKE_CLAUDE_SCENARIO;
	else process.env.FAKE_CLAUDE_SCENARIO = original_scenario;
	if (original_version === undefined) delete process.env.FAKE_CLAUDE_VERSION;
	else process.env.FAKE_CLAUDE_VERSION = original_version;
});

function get_engine(fake: FakeClaudeQuery, options: Record<string, unknown> = {}) {
	return Effect.runPromise(
		ClaudeEngine.pipe(
			Effect.provide(
				make_claude_engine_layer({
					executable: process.execPath,
					executable_args: [executable],
					...options,
				}).pipe(Layer.provide(EngineProcessFactoryLive), Layer.provide(fake.layer)),
			),
		),
	);
}

const open_input = (tag: "start" | "resume" = "start") =>
	tag === "start"
		? {
				_tag: "start" as const,
				artisan_run_id: "claude-run",
				initial_text: "hello",
				working_directory: process.cwd(),
				model: "fake-model",
				provider_options: { "claude.permission_mode": "default" },
			}
		: {
				_tag: "resume" as const,
				artisan_run_id: "claude-resume",
				next_text: "continue",
				working_directory: process.cwd(),
				resume_token: { native_thread_id: "resume-session" },
			};

function init_frame(session: FakeClaudeSession) {
	return { ...claude_init_frame, session_id: session.session_id() } as unknown as SDKMessage;
}

function result_frame(session: FakeClaudeSession, overrides: Record<string, unknown> = {}) {
	return {
		type: "result",
		subtype: "success",
		duration_ms: 5,
		duration_api_ms: 4,
		is_error: false,
		num_turns: 1,
		result: "done",
		stop_reason: "end_turn",
		session_id: session.session_id(),
		total_cost_usd: 0.01,
		usage: { input_tokens: 10, output_tokens: 2 },
		uuid: "3d8a2f5c-0b47-4a3e-9a3e-6c2f6f4d5a01",
		...overrides,
	} as unknown as SDKMessage;
}

function assistant_frame(session: FakeClaudeSession) {
	return {
		...claude_assistant_text_frame,
		session_id: session.session_id(),
	} as unknown as SDKMessage;
}

const completed_script = (session: FakeClaudeSession) => {
	session.emit(init_frame(session));
	session.emit({
		...claude_text_delta_frame,
		session_id: session.session_id(),
	} as unknown as SDKMessage);
	session.emit(assistant_frame(session));
	session.emit(result_frame(session));
};

function failure_from<A>(exit: Exit.Exit<A, unknown>) {
	return Exit.isFailure(exit) ? Cause.squash(exit.cause) : undefined;
}

const wait_until = (predicate: () => boolean) =>
	Effect.gen(function* () {
		for (let attempt = 0; attempt < 500 && !predicate(); attempt += 1) {
			yield* Effect.sleep("10 millis");
		}
		expect(predicate()).toBe(true);
	});

describe("Claude Agent SDK engine", () => {
	it("declares truthful capabilities", () => {
		expect(ClaudeEngineDescriptor.transport).toBe("claude-agent-sdk");
		expect(ClaudeEngineDescriptor.capabilities.approval.state).toBe("supported");
		expect(ClaudeEngineDescriptor.capabilities.steer.state).toBe("experimental");
		expect(ClaudeEngineDescriptor.capabilities.global_guidance.state).toBe("unsupported");
		expect(ClaudeEngineDescriptor.capabilities.model_selection.state).toBe("supported");
		expect(ClaudeEngineDescriptor.capabilities.resume.state).toBe("supported");
	});

	it("is explicitly registrable by its provider id", async () => {
		const engine = await get_engine(make_fake_claude_query(completed_script));
		const resolved = await Effect.runPromise(
			Effect.gen(function* () {
				const registry = yield* EngineRegistry;
				return yield* registry.Get("claude");
			}).pipe(Effect.provide(make_engine_registry_layer([engine]))),
		);
		expect(resolved.Descriptor.id).toBe("claude");
	});

	it("probes saved auth, passes exact SDK options, and streams the initial user turn", async () => {
		const fake = make_fake_claude_query(completed_script);
		const engine = await get_engine(fake);
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open(open_input());
					return {
						events: yield* run.Events.pipe(Stream.runCollect),
						native_thread_id: run.native_thread_id,
					};
				}),
			),
		);
		const events = [...result.events];
		const session = fake.sessions[0]!;

		expect(result.native_thread_id).toMatch(/^[0-9a-f-]{36}$/);
		expect(session.options).toMatchObject({
			cwd: process.cwd(),
			includePartialMessages: true,
			model: "fake-model",
			permissionMode: "default",
			sessionId: result.native_thread_id,
			systemPrompt: { type: "preset", preset: "claude_code" },
		});
		expect(session.options?.resume).toBeUndefined();
		expect(session.received[0]?.message.content).toBe("hello");
		expect(events.some((event) => event._tag === "agent_message_delta")).toBe(true);
		expect(events.filter((event) => event._tag === "run_terminal")).toEqual([
			expect.objectContaining({ state: "completed" }),
		]);
		expect(events.at(-1)).toMatchObject({ _tag: "run_terminal", state: "completed" });
		expect(
			events.every(
				(event) =>
					event._tag === "run_terminal" || event.raw.transport === "claude-agent-sdk",
			),
		).toBe(true);
	});

	it("resumes with the supplied session identity", async () => {
		const fake = make_fake_claude_query(completed_script);
		const engine = await get_engine(fake);
		const native_thread_id = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open(open_input("resume"));
					yield* run.Events.pipe(Stream.runDrain);
					return run.native_thread_id;
				}),
			),
		);
		expect(native_thread_id).toBe("resume-session");
		expect(fake.sessions[0]?.options?.resume).toBe("resume-session");
		expect(fake.sessions[0]?.options?.sessionId).toBeUndefined();
		expect(fake.sessions[0]?.received[0]?.message.content).toBe("continue");
	});

	it("preserves ordered text/image parts as native content blocks", async () => {
		const fake = make_fake_claude_query(completed_script);
		const engine = await get_engine(fake);
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open({
						_tag: "start",
						artisan_run_id: "claude-image-run",
						initial_text: "hello",
						working_directory: process.cwd(),
						initial_content: [
							{ text: "hello", type: "text" },
							{
								bytes: new Uint8Array([1, 2, 3]),
								id: "image-1",
								media_type: "image/png",
								name: "shot.png",
								type: "image",
							},
						],
					});
					yield* run.Events.pipe(Stream.runDrain);
				}),
			),
		);

		expect(fake.sessions[0]?.received[0]?.message.content).toEqual([
			{ text: "hello", type: "text" },
			{
				source: { data: "AQID", media_type: "image/png", type: "base64" },
				type: "image",
			},
		]);
	});

	it("does not fabricate an empty user turn when resume text is absent", async () => {
		const fake = make_fake_claude_query(completed_script);
		const engine = await get_engine(fake);
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open({
						_tag: "resume",
						artisan_run_id: "claude-empty-resume",
						working_directory: process.cwd(),
						resume_token: { native_thread_id: "resume-session" },
					});
					yield* run.Events.pipe(Stream.runDrain);
				}),
			),
		);
		expect(fake.sessions[0]?.received).toHaveLength(0);
	});

	it("maps provider options onto SDK tool and prompt configuration", async () => {
		const fake = make_fake_claude_query(completed_script);
		const engine = await get_engine(fake);
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open({
						...open_input(),
						product_instructions: {
							content: "Follow Artisan product policy.",
							source: "artisan-product",
						},
						provider_options: {
							"claude.append_system_prompt_file": "C:\\workspace\\CLAUDE.md",
							"claude.disable_tools": true,
							"claude.permission_mode": "plan",
							"claude.safe_mode": true,
						},
					});
					yield* run.Events.pipe(Stream.runDrain);
				}),
			),
		);
		expect(fake.sessions[0]?.options).toMatchObject({
			extraArgs: {
				"append-system-prompt-file": "C:\\workspace\\CLAUDE.md",
				"safe-mode": null,
			},
			permissionMode: "plan",
			systemPrompt: {
				append: "Follow Artisan product policy.",
				preset: "claude_code",
				type: "preset",
			},
			tools: [],
		});
	});

	it("maps the canonical permission policy onto native permission modes", async () => {
		const fake = make_fake_claude_query(completed_script);
		const engine = await get_engine(fake);
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const bypass = yield* engine.Open({
						...open_input(),
						provider_options: {},
						permission_policy: {
							approval: "never",
							network_access: true,
							write_access: true,
						},
					});
					yield* bypass.Events.pipe(Stream.runDrain);
					const prompted = yield* engine.Open({
						...open_input(),
						provider_options: {},
						permission_policy: {
							approval: "on_request",
							network_access: true,
							write_access: true,
						},
					});
					yield* prompted.Events.pipe(Stream.runDrain);
				}),
			),
		);

		expect(fake.sessions[0]?.options).toMatchObject({
			allowDangerouslySkipPermissions: true,
			permissionMode: "bypassPermissions",
		});
		expect(fake.sessions[0]?.options?.canUseTool).toBeUndefined();
		expect(fake.sessions[1]?.options).toMatchObject({ permissionMode: "default" });
		expect(fake.sessions[1]?.options?.canUseTool).toBeDefined();
	});

	it("rejects isolation promises this transport cannot keep", async () => {
		const engine = await get_engine(make_fake_claude_query(completed_script));
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.exit(
					engine.Open({
						...open_input(),
						provider_options: {},
						permission_policy: {
							approval: "on_request",
							network_access: false,
							write_access: false,
						},
					}),
				),
			),
		);
		expect(failure_from(result)).toMatchObject({
			_tag: "EngineConfigurationError",
			option: "permission_policy.write_access",
		});
	});

	it("rejects unsupported global guidance before starting a session", async () => {
		let spawn_count = 0;
		const factory = Layer.succeed(EngineProcessFactory, {
			Spawn: () => {
				spawn_count += 1;
				return Effect.die("spawned");
			},
		});
		const engine = await Effect.runPromise(
			ClaudeEngine.pipe(
				Effect.provide(
					make_claude_engine_layer({ executable }).pipe(
						Layer.provide(factory),
						Layer.provide(make_unstartable_claude_query()),
					),
				),
			),
		);
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.exit(
					engine.Open({
						...open_input(),
						global_guidance: {
							content: "Guidance",
							source_file: "C:\\workspace\\AGENTS.md",
						},
					}),
				),
			),
		);

		expect(Exit.isFailure(result)).toBe(true);
		expect(JSON.stringify(result)).toContain("EngineUnsupportedOperationError");
		expect(spawn_count).toBe(0);
	});

	it("bridges native permission prompts to canonical approvals", async () => {
		const decisions: Array<unknown> = [];
		const fake = make_fake_claude_query(async (session) => {
			session.emit(init_frame(session));
			decisions.push(
				await session.request_permission(
					"Bash",
					{ command: "printf ok" },
					{ title: "Run printf ok?" },
				),
			);
			session.emit(result_frame(session));
		});
		const engine = await get_engine(fake);
		const collected: Array<EngineObservation> = [];
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open({
						...open_input(),
						provider_options: {},
						permission_policy: {
							approval: "on_request",
							network_access: true,
							write_access: true,
						},
					});
					const fiber = yield* Effect.forkChild(
						run.Events.pipe(
							Stream.runForEach((event) =>
								Effect.sync(() => {
									collected.push(event);
								}),
							),
						),
					);
					yield* wait_until(() =>
						collected.some(
							(event) => event._tag === "approval" && event.state === "requested",
						),
					);
					const requested = collected.find((event) => event._tag === "approval");
					yield* run.Send({
						_tag: "respond_approval",
						approval_id:
							requested !== undefined && "approval_id" in requested
								? requested.approval_id
								: "missing",
						approved: true,
						command_id: "approve-1",
					});
					yield* Fiber.join(fiber);
				}),
			),
		);

		expect(decisions).toEqual([{ behavior: "allow" }]);
		const approvals = collected.filter((event) => event._tag === "approval");
		expect(approvals[0]).toMatchObject({
			description: "Run printf ok?",
			request: { command: "printf ok", kind: "command" },
			state: "requested",
		});
		expect(approvals[1]).toMatchObject({ approved: true, state: "resolved" });
		expect(collected.at(-1)).toMatchObject({ _tag: "run_terminal", state: "completed" });
	});

	it("denies approvals and fails unknown approval targets", async () => {
		const decisions: Array<unknown> = [];
		const fake = make_fake_claude_query(async (session) => {
			session.emit(init_frame(session));
			decisions.push(await session.request_permission("Edit", { file_path: "a.ts" }));
			session.emit(result_frame(session));
		});
		const engine = await get_engine(fake);
		const collected: Array<EngineObservation> = [];
		const unknown_target = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open({
						...open_input(),
						provider_options: {},
						permission_policy: {
							approval: "on_request",
							network_access: true,
							write_access: true,
						},
					});
					const fiber = yield* Effect.forkChild(
						run.Events.pipe(
							Stream.runForEach((event) =>
								Effect.sync(() => {
									collected.push(event);
								}),
							),
						),
					);
					yield* wait_until(() =>
						collected.some(
							(event) => event._tag === "approval" && event.state === "requested",
						),
					);
					const requested = collected.find((event) => event._tag === "approval");
					const unknown = yield* Effect.exit(
						run.Send({
							_tag: "respond_approval",
							approval_id: "claude:missing:approval:99",
							approved: true,
							command_id: "approve-unknown",
						}),
					);
					yield* run.Send({
						_tag: "respond_approval",
						approval_id:
							requested !== undefined && "approval_id" in requested
								? requested.approval_id
								: "missing",
						approved: false,
						command_id: "approve-2",
					});
					yield* Fiber.join(fiber);
					return unknown;
				}),
			),
		);

		expect(decisions).toEqual([
			{ behavior: "deny", message: "The Artisan user denied this action" },
		]);
		expect(failure_from(unknown_target)).toMatchObject({
			_tag: "EngineCommandTargetError",
			target: "approval",
			target_id: "claude:missing:approval:99",
		});
		const approvals = collected.filter((event) => event._tag === "approval");
		expect(approvals[0]).toMatchObject({ request: { kind: "file_change" } });
	});

	it("steers the active turn through stream input", async () => {
		const fake = make_fake_claude_query(async (session) => {
			session.emit(init_frame(session));
			for (let attempt = 0; attempt < 500 && session.received.length < 2; attempt += 1) {
				await new Promise((resolve) => setTimeout(resolve, 10));
			}
			session.emit(result_frame(session));
		});
		const engine = await get_engine(fake);
		const events = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open(open_input());
					yield* run.Send({ _tag: "steer", command_id: "steer-1", text: "also this" });
					return yield* run.Events.pipe(Stream.runCollect);
				}),
			),
		);

		expect([...events].at(-1)).toMatchObject({ _tag: "run_terminal", state: "completed" });
		expect(fake.sessions[0]?.received.map((message) => message.message.content)).toEqual([
			"hello",
			"also this",
		]);
	});

	it("supports cancel with exactly one terminal and closes the session", async () => {
		const fake = make_fake_claude_query((session) => {
			session.emit(init_frame(session));
		});
		const engine = await get_engine(fake);
		const events = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open(open_input());
					yield* run.Send({ _tag: "cancel", command_id: "cancel" });
					return yield* run.Events.pipe(Stream.runCollect);
				}),
			),
		);

		expect([...events].filter((event) => event._tag === "run_terminal")).toEqual([
			expect.objectContaining({ state: "cancelled" }),
		]);
		expect(fake.sessions[0]?.closed()).toBe(true);
	});

	it("fails semantically on an error result and rejects duplicate command intents", async () => {
		const fake = make_fake_claude_query((session) => {
			session.emit(init_frame(session));
			session.emit(
				result_frame(session, {
					is_error: true,
					subtype: "error_during_execution",
				}),
			);
		});
		const engine = await get_engine(fake);
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open(open_input());
					const events = yield* run.Events.pipe(Stream.runCollect);
					const question = yield* Effect.exit(
						run.Send({
							_tag: "respond_question",
							answers: {},
							command_id: "question-1",
						}),
					);
					return { events, question };
				}),
			),
		);

		expect([...result.events].at(-1)).toMatchObject({ _tag: "run_terminal", state: "failed" });
		expect(failure_from(result.question)).toMatchObject({ _tag: "EngineRunClosedError" });
	});

	it("fails when the stream ends without a result frame", async () => {
		const fake = make_fake_claude_query((session) => {
			session.emit(init_frame(session));
			session.emit(assistant_frame(session));
			session.end();
		});
		const engine = await get_engine(fake);
		const events = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open(open_input());
					return yield* run.Events.pipe(Stream.runCollect);
				}),
			),
		);

		expect(
			[...events].some(
				(event) =>
					event._tag === "process_diagnostic" &&
					event.message.includes("without a result frame"),
			),
		).toBe(true);
		expect([...events].at(-1)).toMatchObject({ _tag: "run_terminal", state: "failed" });
	});

	it("fails deterministically on session identity mismatch", async () => {
		const fake = make_fake_claude_query((session) => {
			session.emit({
				...claude_init_frame,
				session_id: "not-the-run-session",
			} as unknown as SDKMessage);
			session.emit(result_frame(session));
		});
		const engine = await get_engine(fake);
		const events = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open(open_input());
					return yield* run.Events.pipe(Stream.runCollect);
				}),
			),
		);

		expect([...events].at(-1)).toMatchObject({ _tag: "run_terminal", state: "failed" });
		expect(
			[...events].some(
				(event) =>
					event._tag === "process_diagnostic" &&
					event.message === "Claude session identity mismatch",
			),
		).toBe(true);
	});

	it("settles a silent session as stalled through the inactivity deadline", async () => {
		const fake = make_fake_claude_query((session) => {
			session.emit(init_frame(session));
		});
		const engine = await get_engine(fake, { inactivity_ms: 100 });
		const events = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const run = yield* engine.Open(open_input());
					return yield* run.Events.pipe(Stream.runCollect);
				}),
			),
		);

		expect([...events].at(-1)).toMatchObject({ _tag: "run_terminal", state: "failed" });
		expect(
			[...events].some(
				(event) => event._tag === "process_diagnostic" && event.message.includes("stalled"),
			),
		).toBe(true);
	});

	it("version-tests native continuation and permits target-model changes only on 2.1.220", async () => {
		const fake = make_fake_claude_query(completed_script);
		const engine = await get_engine(fake);
		const exact = await Effect.runPromise(
			engine.CheckNativeContinuation!({
				resume_token: { native_thread_id: "resume-session" },
				source_model: "claude-sonnet-4",
				target_model: "claude-opus-4",
			}),
		);
		expect(exact).toEqual({ state: "compatible" });
		const missing_target = await Effect.runPromise(
			engine.CheckNativeContinuation!({
				resume_token: { native_thread_id: "resume-session" },
			}),
		);
		expect(missing_target).toMatchObject({ state: "incompatible" });
		process.env.FAKE_CLAUDE_VERSION = "2.1.219";
		const old_engine = await get_engine(fake);
		const old = await Effect.runPromise(
			old_engine.CheckNativeContinuation!({
				resume_token: { native_thread_id: "resume-session" },
			}),
		);
		expect(old).toMatchObject({ state: "unsupported" });
	});

	const live_probe = process.env.ARTISAN_ENGINE_LIVE === "1" ? it : it.skip;
	live_probe(
		"runs only the non-billable live version/auth probe",
		async () => {
			const engine = await get_engine(make_fake_claude_query(completed_script), {
				executable: "claude",
				version_timeout_ms: 30_000,
				auth_timeout_ms: 30_000,
			});
			const probe = await Effect.runPromise(engine.Probe({ client_name: "artisan-tests" }));
			expect(probe.version).toMatch(/\d+\.\d+\.\d+/);
		},
		70_000,
	);
});
