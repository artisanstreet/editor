import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it } from "vitest";
import { Cause, Deferred, Effect, Exit, Fiber, Layer, Ref, Scope, Stream } from "effect";

import {
	CodexEngine,
	CodexProcessFactoryLive,
	make_codex_engine_layer,
	type EngineObservation,
} from "@artisan/engines";
import { MakeCodexAppServerEventBuffer } from "../../modules/engines/src/codex/internal/app-server-event-buffer";

const fixture_path = fileURLToPath(new URL("./fixtures/fake-app-server.ts", import.meta.url));
const original_pid_file = process.env.FAKE_APP_SERVER_PID_FILE;
const original_request_file = process.env.FAKE_APP_SERVER_REQUEST_FILE;
const original_scenario = process.env.FAKE_APP_SERVER_SCENARIO;

afterEach(() => {
	if (original_scenario === undefined) {
		delete process.env.FAKE_APP_SERVER_SCENARIO;
	} else {
		process.env.FAKE_APP_SERVER_SCENARIO = original_scenario;
	}

	if (original_pid_file === undefined) {
		delete process.env.FAKE_APP_SERVER_PID_FILE;
	} else {
		process.env.FAKE_APP_SERVER_PID_FILE = original_pid_file;
	}

	if (original_request_file === undefined) {
		delete process.env.FAKE_APP_SERVER_REQUEST_FILE;
	} else {
		process.env.FAKE_APP_SERVER_REQUEST_FILE = original_request_file;
	}
});

function make_layer(
	options: {
		readonly event_capacity?: number;
	} = {},
) {
	return make_codex_engine_layer({
		...options,
		executable: process.execPath,
		executable_args: [fixture_path],
	}).pipe(Layer.provide(CodexProcessFactoryLive));
}

function terminals(events: ReadonlyArray<EngineObservation>) {
	return events.filter((event) => event._tag === "run_terminal");
}

function is_process_alive(pid: number) {
	try {
		process.kill(pid, 0);

		return true;
	} catch {
		return false;
	}
}

async function read_pid(path: string) {
	for (let attempt = 0; attempt < 40; attempt += 1) {
		try {
			return Number(await readFile(path, "utf8"));
		} catch {
			await new Promise<void>((resolve) => setTimeout(resolve, 25));
		}
	}

	throw new Error(`Timed out waiting for fixture pid at ${path}`);
}

async function wait_for_process_exit(pid: number) {
	for (let attempt = 0; attempt < 40; attempt += 1) {
		if (!is_process_alive(pid)) {
			return;
		}

		await new Promise<void>((resolve) => setTimeout(resolve, 50));
	}
}

describe("Codex engine run", () => {
	it("validates explicit native model continuation", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "continuation";

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;

					return yield* engine.CheckNativeContinuation!({
						resume_token: { native_thread_id: "thread-source" },
						target_model: "gpt-5-mini",
					});
				}),
			).pipe(Effect.provide(make_layer())),
		);

		expect(result).toEqual({ state: "compatible" });
	});

	it("rejects a target model absent from the current Codex catalog", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "continuation";
		const output = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;
					return yield* engine.CheckNativeContinuation!({
						resume_token: { native_thread_id: "thread-source" },
						target_model: "missing",
					});
				}),
			).pipe(Effect.provide(make_layer())),
		);
		expect(output.state).toBe("incompatible");
	});

	it("rejects native continuation without an explicit target model", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "continuation";
		const output = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;
					return yield* engine.CheckNativeContinuation!({
						resume_token: { native_thread_id: "thread-source" },
					});
				}),
			).pipe(Effect.provide(make_layer())),
		);
		expect(output).toMatchObject({ state: "incompatible" });
	});

	it("follows bounded Codex model catalog pagination", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "continuation-model-pagination";
		const output = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;
					return yield* engine.CheckNativeContinuation!({
						resume_token: { native_thread_id: "thread-source" },
						target_model: "gpt-later",
					});
				}),
			).pipe(Effect.provide(make_layer())),
		);
		expect(output).toEqual({ state: "compatible" });
	});

	it.each(["version-older", "version-newer"])(
		"rejects continuation operations outside the pinned Codex protocol version: %s",
		async (scenario) => {
			process.env.FAKE_APP_SERVER_SCENARIO = scenario;
			await expect(
				Effect.runPromise(
					Effect.scoped(
						Effect.gen(function* () {
							const engine = yield* CodexEngine;
							return yield* engine.CheckNativeContinuation!({
								resume_token: { native_thread_id: "thread-source" },
								target_model: "gpt-5",
							});
						}),
					).pipe(Effect.provide(make_layer())),
				),
			).rejects.toBeDefined();
		},
	);

	it("starts a real child, preserves ordered raw provenance, and produces one terminal", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "complete";

		const events = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;
					const run = yield* engine.Open({
						_tag: "start",
						artisan_run_id: "run-complete",
						initial_text: "Hello",
						model: "gpt-5",
						permission_policy: {
							approval: "on_request",
							network_access: false,
							write_access: true,
						},
						working_directory: "C:\\workspace",
					});

					return yield* run.Events.pipe(Stream.runCollect);
				}),
			).pipe(Effect.provide(make_layer())),
		);
		const collected = [...events];

		expect(collected.map((event) => event.sequence)).toEqual(
			[...collected.keys()].map((index) => index + 1),
		);
		expect(collected.find((event) => event._tag === "agent_message_delta")).toMatchObject({
			raw: {
				native_method: "item/agentMessage/delta",
				protocol_version: "v1",
				raw_frame_base64: expect.any(String),
			},
		});
		expect(terminals(collected)).toEqual([expect.objectContaining({ state: "completed" })]);
	});

	it("maps global guidance natively for thread start and resume without changing user text", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-codex-requests-"));
		const request_path = join(directory, "thread-requests.jsonl");

		process.env.FAKE_APP_SERVER_REQUEST_FILE = request_path;
		process.env.FAKE_APP_SERVER_SCENARIO = "complete";

		try {
			await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const engine = yield* CodexEngine;
						const started = yield* engine.Open({
							_tag: "start",
							artisan_run_id: "permission-start",
							global_guidance: {
								content: "Use project guidance.",
								source_file: "C:\\workspace\\AGENTS.md",
							},
							initial_text: "Start",
							model: "gpt-5",
							permission_policy: {
								approval: "never",
								network_access: true,
								write_access: true,
							},
							provider_options: {
								"codex.reasoning_effort": "high",
								"codex.service_tier": "fast",
							},
							working_directory: "C:\\workspace",
						});

						yield* started.Events.pipe(Stream.runDrain);

						const resumed = yield* engine.Open({
							_tag: "resume",
							artisan_run_id: "permission-resume",
							global_guidance: {
								content: "Use project guidance.",
								source_file: "C:\\workspace\\AGENTS.md",
							},
							next_content: [
								{ text: "Resume with ", type: "text" },
								{
									bytes: new Uint8Array([1, 2, 3]),
									id: "resume-image",
									media_type: "image/png",
									name: "resume.png",
									type: "image",
								},
							],
							permission_policy: {
								approval: "on_request",
								network_access: false,
								write_access: false,
							},
							resume_token: { native_thread_id: "thread-resumed" },
							working_directory: "C:\\workspace",
						});

						yield* resumed.Send({
							_tag: "close",
							command_id: "close-permission-resume",
						});
						yield* resumed.Events.pipe(Stream.runDrain);
					}),
				).pipe(Effect.provide(make_layer())),
			);

			const requests = (await readFile(request_path, "utf8"))
				.trim()
				.split("\n")
				.map((line) => JSON.parse(line));

			expect(requests).toEqual([
				{
					method: "thread/start",
					params: {
						approvalPolicy: "never",
						config: {
							model_reasoning_effort: "high",
							model_reasoning_summary: "auto",
							sandbox_workspace_write: { network_access: true },
						},
						cwd: "C:\\workspace",
						developerInstructions: "Use project guidance.",
						model: "gpt-5",
						sandbox: "workspace-write",
						serviceTier: "fast",
					},
				},
				{
					method: "turn/start",
					params: {
						input: [{ text: "Start", text_elements: [], type: "text" }],
						serviceTier: "fast",
						threadId: "thread-started",
					},
				},
				{
					method: "thread/resume",
					params: {
						approvalPolicy: "on-request",
						config: { model_reasoning_summary: "auto" },
						cwd: "C:\\workspace",
						developerInstructions: "Use project guidance.",
						sandbox: "read-only",
						threadId: "thread-resumed",
					},
				},
				{
					method: "turn/start",
					params: {
						input: [
							{
								text: "Resume with ",
								text_elements: [],
								type: "text",
							},
							{
								type: "image",
								url: "data:image/png;base64,AQID",
							},
						],
						threadId: "thread-resumed",
					},
				},
			]);
		} finally {
			await rm(directory, { force: true, recursive: true });
		}
	});

	it("sends Full access through the app-server transport as danger-full-access", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-codex-full-access-"));
		const request_path = join(directory, "thread-requests.jsonl");

		process.env.FAKE_APP_SERVER_REQUEST_FILE = request_path;
		process.env.FAKE_APP_SERVER_SCENARIO = "complete";

		try {
			await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const engine = yield* CodexEngine;
						const run = yield* engine.Open({
							_tag: "start",
							artisan_run_id: "full-access-start",
							initial_text: "Create a sibling repository.",
							permission_policy: {
								approval: "never",
								edit_scope: "host",
								network_access: true,
								write_access: true,
							},
							working_directory: "C:\\workspace",
						});

						yield* run.Events.pipe(Stream.runDrain);
					}),
				).pipe(Effect.provide(make_layer())),
			);

			const requests = (await readFile(request_path, "utf8"))
				.trim()
				.split("\n")
				.map((line) => JSON.parse(line));

			expect(requests[0]).toEqual({
				method: "thread/start",
				params: {
					approvalPolicy: "never",
					config: { model_reasoning_summary: "auto" },
					cwd: "C:\\workspace",
					sandbox: "danger-full-access",
				},
			});
		} finally {
			await rm(directory, { force: true, recursive: true });
		}
	});

	it.each([
		{
			label: "approval always",
			metadata: {
				permission_policy: {
					approval: "always",
					network_access: false,
					write_access: true,
				},
			},
			option: "permission_policy.approval",
		},
		{
			label: "network access in read-only mode",
			metadata: {
				permission_policy: {
					approval: "never",
					network_access: true,
					write_access: false,
				},
			},
			option: "permission_policy.network_access",
		},
		{
			label: "an unknown provider option",
			metadata: {
				provider_options: { "codex.unknown": true },
			},
			option: "provider_options.codex.unknown",
		},
		{
			label: "an exec-only provider option on app-server",
			metadata: {
				provider_options: { "codex.exec.profile": "fixture-profile" },
			},
			option: "provider_options.codex.exec.profile",
		},
		{
			label: "an empty global guidance source file",
			metadata: {
				global_guidance: { content: "Guidance", source_file: "" },
			},
			option: "global_guidance.source_file",
		},
	] as const)("rejects $label before spawning or requesting", async ({ metadata, option }) => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-codex-policy-reject-"));
		const pid_path = join(directory, "app-server.pid");
		const request_path = join(directory, "thread-requests.jsonl");

		process.env.FAKE_APP_SERVER_PID_FILE = pid_path;
		process.env.FAKE_APP_SERVER_REQUEST_FILE = request_path;
		process.env.FAKE_APP_SERVER_SCENARIO = "complete";

		try {
			const opened = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const engine = yield* CodexEngine;

						return yield* engine
							.Open({
								_tag: "start",
								artisan_run_id: `policy-reject-${option}`,
								initial_text: "Must not reach Codex",
								...metadata,
								working_directory: "C:\\workspace",
							})
							.pipe(Effect.exit);
					}),
				).pipe(Effect.provide(make_layer())),
			);
			const failure = Exit.isFailure(opened) ? Cause.squash(opened.cause) : undefined;

			expect(failure).toMatchObject({
				_tag: "EngineConfigurationError",
				option,
			});
			expect(existsSync(pid_path)).toBe(false);
			expect(existsSync(request_path)).toBe(false);
		} finally {
			await rm(directory, { force: true, recursive: true });
		}
	});

	it("assigns deterministic unique ids to repeated process diagnostics", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "run-diagnostics";

		const events = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;
					const run = yield* engine.Open({
						_tag: "start",
						artisan_run_id: "run-diagnostics",
						initial_text: "Diagnose",
						working_directory: "C:\\workspace",
					});

					return yield* run.Events.pipe(Stream.runCollect);
				}),
			).pipe(Effect.provide(make_layer())),
		);
		const diagnostics = [...events].filter((event) => event._tag === "process_diagnostic");

		expect(diagnostics.length).toBeGreaterThanOrEqual(2);
		expect(new Set(diagnostics.map((event) => event.observation_id)).size).toBe(
			diagnostics.length,
		);
		expect(terminals([...events])).toEqual([expect.objectContaining({ state: "completed" })]);
	});

	it("resolves approval and multi-question requests with stable idempotency", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "requests";

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;
					const request_count = yield* Ref.make(0);
					const requests_ready = yield* Deferred.make<void>();
					const run = yield* engine.Open({
						_tag: "resume",
						artisan_run_id: "run-resume",
						next_text: "Continue",
						resume_token: { native_thread_id: "thread-resumed" },
						working_directory: "C:\\workspace",
					});
					const events_fiber = yield* run.Events.pipe(
						Stream.tap((event) => {
							const is_request =
								(event._tag === "approval" || event._tag === "question") &&
								event.state === "requested";

							return is_request
								? Ref.updateAndGet(request_count, (count) => count + 1).pipe(
										Effect.flatMap((count) =>
											count === 3
												? Deferred.succeed(requests_ready, undefined).pipe(
														Effect.ignore,
													)
												: Effect.void,
										),
									)
								: Effect.void;
						}),
						Stream.runCollect,
						Effect.forkChild,
					);

					yield* Deferred.await(requests_ready);
					const partial = yield* run
						.Send({
							_tag: "respond_question",
							answers: { "question-1": ["one"] },
							command_id: "partial-questions",
						})
						.pipe(Effect.exit);

					yield* run.Send({
						_tag: "respond_approval",
						approval_id: "approval-request",
						approved: true,
						command_id: "approve",
					});
					yield* run.Send({
						_tag: "respond_question",
						answers: { "question-1": ["one"], "question-2": ["two"] },
						command_id: "questions",
					});
					yield* run.Send({
						_tag: "respond_question",
						answers: { "question-2": ["two"], "question-1": ["one"] },
						command_id: "questions",
					});
					const conflict = yield* run
						.Send({
							_tag: "respond_question",
							answers: { "question-1": ["changed"], "question-2": ["two"] },
							command_id: "questions",
						})
						.pipe(Effect.exit);
					const events = yield* Fiber.join(events_fiber);

					return { conflict, events: [...events], partial };
				}),
			).pipe(Effect.provide(make_layer())),
		);
		const questions = result.events.filter((event) => event._tag === "question");
		const approvals = result.events.filter((event) => event._tag === "approval");
		const conflict = Exit.isFailure(result.conflict)
			? Cause.squash(result.conflict.cause)
			: undefined;
		const partial = Exit.isFailure(result.partial)
			? Cause.squash(result.partial.cause)
			: undefined;

		expect(questions).toMatchObject([
			{ question_id: "question-1", state: "requested" },
			{ question_id: "question-2", state: "requested" },
			{ answers: ["one"], question_id: "question-1", state: "resolved" },
			{ answers: ["two"], question_id: "question-2", state: "resolved" },
		]);
		expect(approvals).toMatchObject([
			{ approval_id: "approval-request", state: "requested" },
			{ approval_id: "approval-request", approved: true, state: "resolved" },
		]);
		expect(conflict).toMatchObject({ _tag: "EngineCommandIdConflictError" });
		expect(partial).toMatchObject({
			_tag: "EngineCommandTargetError",
			target_id: "incomplete-request-group",
		});
		expect(new Set(result.events.map((event) => event.observation_id)).size).toBe(
			result.events.length,
		);
		expect(terminals(result.events)).toEqual([expect.objectContaining({ state: "completed" })]);
	});

	it("discovers an already active turn and preserves opaque resume state", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "resume-active";

		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const engine = yield* CodexEngine;
				const owner_scope = yield* Scope.make();
				const run = yield* engine
					.Open({
						_tag: "resume",
						artisan_run_id: "run-active-resume",
						resume_token: {
							native_thread_id: "thread-resumed",
							opaque_checkpoint: "opaque-state",
						},
						working_directory: "C:\\workspace",
					})
					.pipe(Scope.provide(owner_scope));

				yield* run.Send({ _tag: "steer", command_id: "steer", text: "More detail" });
				yield* run.Send({ _tag: "close", command_id: "close" });

				return {
					events: yield* run.Events.pipe(Stream.runCollect),
					resume_token: run.resume_token,
				};
			}).pipe(Effect.provide(make_layer())),
		);

		expect(terminals([...result.events])).toEqual([
			expect.objectContaining({ state: "closed" }),
		]);
		expect(result.resume_token).toEqual({
			native_thread_id: "thread-resumed",
			opaque_checkpoint: "opaque-state",
		});
	});

	it("steers the new turn started with resumed next text", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "resume-active-next-text";

		const steer = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;
					const run = yield* engine.Open({
						_tag: "resume",
						artisan_run_id: "run-active-resume-next-text",
						next_text: "Start a new turn",
						resume_token: { native_thread_id: "thread-resumed" },
						working_directory: "C:\\workspace",
					});
					const result = yield* run
						.Send({ _tag: "steer", command_id: "steer-new-turn", text: "Continue" })
						.pipe(Effect.exit);

					yield* run.Send({ _tag: "close", command_id: "close-new-turn" });

					return result;
				}),
			).pipe(Effect.provide(make_layer())),
		);

		expect(Exit.isSuccess(steer)).toBe(true);
	});

	it("ignores stale turn completion when a newer native turn is active", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "stale-turn";

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;
					const newer_turn_ready = yield* Deferred.make<void>();
					const stale_completion_ready = yield* Deferred.make<void>();
					const run = yield* engine.Open({
						_tag: "start",
						artisan_run_id: "run-stale-turn",
						initial_text: "Start",
						working_directory: "C:\\workspace",
					});
					const events_fiber = yield* run.Events.pipe(
						Stream.tap((event) => {
							if (event._tag !== "turn_state") {
								return Effect.void;
							}

							if (event.state === "started" && event.turn_id === "turn-newer") {
								return Deferred.succeed(newer_turn_ready, undefined).pipe(
									Effect.ignore,
								);
							}

							return event.state === "completed"
								? Deferred.succeed(stale_completion_ready, undefined).pipe(
										Effect.ignore,
									)
								: Effect.void;
						}),
						Stream.runCollect,
						Effect.forkChild,
					);

					yield* Deferred.await(newer_turn_ready);
					yield* Deferred.await(stale_completion_ready);

					const steer = yield* run
						.Send({ _tag: "steer", command_id: "steer-newer", text: "Continue" })
						.pipe(Effect.exit);

					yield* run.Send({ _tag: "close", command_id: "close-stale-test" });

					return { events: yield* Fiber.join(events_fiber), steer };
				}),
			).pipe(Effect.provide(make_layer())),
		);
		const turn_states = result.events.filter((event) => event._tag === "turn_state");

		expect(Exit.isSuccess(result.steer)).toBe(true);
		expect(turn_states).toEqual(
			expect.arrayContaining([
				expect.objectContaining({ state: "started", turn_id: "turn-newer" }),
				expect.objectContaining({ state: "completed" }),
			]),
		);
		expect(terminals([...result.events])).toEqual([
			expect.objectContaining({ state: "closed" }),
		]);
	});

	it("keeps the root turn open while a native subagent completes", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "subagent-lifecycle";

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;
					const child_completed = yield* Deferred.make<void>();
					const run = yield* engine.Open({
						_tag: "start",
						artisan_run_id: "run-subagent-lifecycle",
						initial_text: "Delegate then continue",
						working_directory: "C:\\workspace",
					});
					const events_fiber = yield* run.Events.pipe(
						Stream.tap((event) =>
							event._tag === "subagent" &&
							event.agent_native_thread_id === "thread-child" &&
							event.state === "completed"
								? Deferred.succeed(child_completed, undefined).pipe(Effect.ignore)
								: Effect.void,
						),
						Stream.runCollect,
						Effect.forkChild,
					);

					yield* Deferred.await(child_completed);
					const steer = yield* run
						.Send({ _tag: "steer", command_id: "root-still-active", text: "Continue" })
						.pipe(Effect.exit);

					return { events: [...(yield* Fiber.join(events_fiber))], steer };
				}),
			).pipe(Effect.provide(make_layer())),
		);

		expect(Exit.isSuccess(result.steer)).toBe(true);
		expect(result.events).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					_tag: "subagent",
					agent_native_thread_id: "thread-child",
					agent_path: "/root/reviewer",
					parent_native_thread_id: "thread-started",
					state: "discovered",
				}),
				expect.objectContaining({
					_tag: "subagent",
					agent_native_thread_id: "thread-child",
					state: "running",
					turn_id: "turn-child",
				}),
				expect.objectContaining({
					_tag: "subagent",
					agent_native_thread_id: "thread-child",
					state: "completed",
					turn_id: "turn-child",
				}),
				expect.objectContaining({
					_tag: "subagent",
					agent_native_thread_id: "thread-grandchild",
					agent_path: "/root/reviewer/checker",
					parent_native_thread_id: "thread-child",
					state: "discovered",
				}),
				expect.objectContaining({
					_tag: "subagent",
					agent_native_thread_id: "thread-grandchild",
					state: "interrupted",
				}),
				expect.objectContaining({ _tag: "agent_message_delta", delta: "Root continued" }),
			]),
		);
		expect(
			result.events.filter(
				(event) =>
					event._tag === "agent_message_completed" &&
					event.message === "Child-only result",
			),
		).toEqual([]);
		expect(result.events).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					_tag: "subagent_transcript",
					agent_native_thread_id: "thread-child",
					content: expect.objectContaining({
						_tag: "agent_message_completed",
						message: "Child-only result",
					}),
					parent_native_thread_id: "thread-started",
				}),
			]),
		);
		const child_transcript = result.events.find(
			(event) =>
				event._tag === "subagent_transcript" &&
				event.agent_native_thread_id === "thread-child",
		);
		expect(child_transcript?.sequence).toBeGreaterThan(0);
		expect(terminals(result.events)).toEqual([expect.objectContaining({ state: "completed" })]);
	});

	it("records a command id before an ambiguous provider failure", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "steer-failure";

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;
					const run = yield* engine.Open({
						_tag: "resume",
						artisan_run_id: "run-steer-failure",
						resume_token: { native_thread_id: "thread-resumed" },
						working_directory: "C:\\workspace",
					});
					const events_fiber = yield* run.Events.pipe(
						Stream.runCollect,
						Effect.forkChild,
					);
					const command = {
						_tag: "steer" as const,
						command_id: "ambiguous-steer",
						text: "Continue once",
					};
					const first = yield* run.Send(command).pipe(Effect.exit);

					yield* Effect.sleep(10);

					const duplicate = yield* run.Send(command).pipe(Effect.exit);

					yield* run.Send({ _tag: "close", command_id: "close-failure-test" });

					return {
						duplicate,
						events: yield* Fiber.join(events_fiber),
						first,
					};
				}),
			).pipe(Effect.provide(make_layer())),
		);
		const deliveries = [...result.events].filter(
			(event) => event.raw.native_method === "fixture/steerReceived",
		);

		expect(Exit.isFailure(result.first)).toBe(true);
		expect(Exit.isSuccess(result.duplicate)).toBe(true);
		expect(deliveries).toHaveLength(1);
	});

	it("turns a child-process crash into exactly one failed terminal", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "run-crash";

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;
					const run = yield* engine.Open({
						_tag: "start",
						artisan_run_id: "run-crash",
						initial_text: "Crash",
						working_directory: "C:\\workspace",
					});
					const events = yield* run.Events.pipe(Stream.runCollect);

					return { events: [...events], terminal: yield* run.Closed };
				}),
			).pipe(Effect.provide(make_layer())),
		);

		expect(result.terminal).toBe("failed");
		expect(terminals(result.events)).toEqual([expect.objectContaining({ state: "failed" })]);
	});

	it("completes a 600-event burst with a deliberately slow canonical consumer", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "event-flood";

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;
					const run = yield* engine.Open({
						_tag: "start",
						artisan_run_id: "run-backpressure",
						initial_text: "Flood",
						working_directory: "C:\\workspace",
					});
					const events = yield* run.Events.pipe(
						Stream.mapEffect((event) => Effect.sleep(1).pipe(Effect.as(event))),
						Stream.runCollect,
					);
					const terminal = yield* run.Closed;

					return { events: [...events], terminal };
				}),
			).pipe(Effect.provide(make_layer({ event_capacity: 4 }))),
		);

		expect(result.terminal).toBe("completed");
		expect(result.events.filter((event) => event._tag === "native_action")).toHaveLength(600);
		expect(terminals(result.events)).toEqual([expect.objectContaining({ state: "completed" })]);
	});

	it.each([
		["cancel", "cancelled"],
		["close", "closed"],
	] as const)(
		"keeps app-server terminal last when a diagnostic emit races %s",
		async (_command, terminal_state) => {
			const result = await Effect.runPromise(
				Effect.gen(function* () {
					const allow_enqueue = yield* Deferred.make<void>();
					const emit_reserved = yield* Deferred.make<void>();
					const finish_started = yield* Deferred.make<void>();
					const close_count = yield* Ref.make(0);
					const buffer = yield* MakeCodexAppServerEventBuffer({
						artisan_run_id: `app-race-${terminal_state}`,
						BeforeEnqueue: () =>
							Deferred.succeed(emit_reserved, undefined).pipe(
								Effect.andThen(Deferred.await(allow_enqueue)),
								Effect.asVoid,
							),
						BeforeFinish: Deferred.succeed(finish_started, undefined).pipe(
							Effect.asVoid,
						),
						capacity: 4,
						CloseSession: Ref.update(close_count, (count) => count + 1),
					});
					const events_fiber = yield* buffer.Events.pipe(
						Stream.runCollect,
						Effect.forkChild,
					);
					const emit_fiber = yield* buffer
						.Emit({
							_tag: "process_diagnostic",
							artisan_run_id: `app-race-${terminal_state}`,
							level: "info",
							message: "diagnostic reserved before terminal",
							observation_id: `app-race-${terminal_state}:diagnostic`,
							raw: {
								engine_id: "codex",
								frame: { source: "diagnostic-race-test" },
								transport: "stdio-jsonl",
							},
							sequence: 0,
						})
						.pipe(Effect.forkChild);

					yield* Deferred.await(emit_reserved);

					const finish_fiber = yield* buffer
						.Finish(terminal_state)
						.pipe(Effect.forkChild);

					yield* Deferred.await(finish_started);
					yield* Deferred.succeed(allow_enqueue, undefined);
					yield* Fiber.join(emit_fiber);
					yield* Fiber.join(finish_fiber);

					const late_emit = yield* buffer
						.Emit({
							_tag: "process_diagnostic",
							artisan_run_id: `app-race-${terminal_state}`,
							level: "info",
							message: "must not follow terminal",
							observation_id: `app-race-${terminal_state}:late`,
							raw: {
								engine_id: "codex",
								frame: { source: "late-race-test" },
								transport: "stdio-jsonl",
							},
							sequence: 0,
						})
						.pipe(Effect.exit);

					return {
						close_count: yield* Ref.get(close_count),
						events: [...(yield* Fiber.join(events_fiber))],
						late_emit,
					};
				}),
			);
			const terminal_events = terminals(result.events);

			expect(result.events.map((event) => event.sequence)).toEqual([1, 2]);
			expect(terminal_events).toEqual([expect.objectContaining({ state: terminal_state })]);
			expect(result.events.at(-1)).toEqual(terminal_events[0]);
			expect(Exit.isFailure(result.late_emit)).toBe(true);
			expect(result.close_count).toBe(1);
		},
	);

	it("rejects invalid event capacity before spawning Codex", async () => {
		const exit = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;

					return yield* engine
						.Open({
							_tag: "start",
							artisan_run_id: "run-invalid-capacity",
							initial_text: "No spawn",
							working_directory: "C:\\workspace",
						})
						.pipe(Effect.exit);
				}),
			).pipe(Effect.provide(make_layer({ event_capacity: 0 }))),
		);
		const error = Exit.isFailure(exit) ? Cause.squash(exit.cause) : undefined;

		expect(error).toMatchObject({
			_tag: "EngineConfigurationError",
			option: "event_capacity",
			value: 0,
		});
	});

	it("emits one closed terminal and kills the child when its owning scope closes", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-engine-"));
		const pid_path = join(directory, "codex.pid");

		process.env.FAKE_APP_SERVER_PID_FILE = pid_path;
		process.env.FAKE_APP_SERVER_SCENARIO = "resume-active";

		try {
			const result = await Effect.runPromise(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;
					const owner_scope = yield* Scope.make();
					const run = yield* engine
						.Open({
							_tag: "resume",
							artisan_run_id: "run-scope-close",
							resume_token: { native_thread_id: "thread-resumed" },
							working_directory: "C:\\workspace",
						})
						.pipe(Scope.provide(owner_scope));
					const events_fiber = yield* run.Events.pipe(
						Stream.runCollect,
						Effect.forkChild,
					);
					const pid = yield* Effect.tryPromise({
						try: () => read_pid(pid_path),
						catch: (cause) => cause,
					});

					yield* Scope.close(owner_scope, Exit.succeed(undefined));

					return {
						events: [...(yield* Fiber.join(events_fiber))],
						pid,
						terminal: yield* run.Closed,
					};
				}).pipe(Effect.provide(make_layer())),
			);

			await wait_for_process_exit(result.pid);

			expect(result.terminal).toBe("closed");
			expect(terminals(result.events)).toEqual([
				expect.objectContaining({ state: "closed" }),
			]);
			expect(is_process_alive(result.pid)).toBe(false);
		} finally {
			await rm(directory, { force: true, recursive: true });
		}
	});
});
