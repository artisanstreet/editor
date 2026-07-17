import { Buffer } from "node:buffer";
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
import { MakeCodexAppServerEventBuffer } from "../../modules/engines/src/codex/internal/codex-app-server-event-buffer";
import { MakeCodexAppServerThreadOptions } from "../../modules/engines/src/codex/internal/codex-permissions";
import { make_transcript_sequence_replay } from "./harness/transcript-process";

const fixture_path = fileURLToPath(new URL("./fixtures/fake-app-server.mjs", import.meta.url));
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
		readonly transport_selection?: "app_server_only" | "prefer_app_server_with_exec_fallback";
	} = {},
) {
	return make_codex_engine_layer({
		...options,
		executable: process.execPath,
		executable_args: [fixture_path],
		transport_selection: options.transport_selection ?? "app_server_only",
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

function jsonl_chunk(at_ms: number, stream: "stdin" | "stdout", payload: unknown) {
	return {
		at_ms,
		chunk_base64: Buffer.from(`${JSON.stringify(payload)}\n`).toString("base64"),
		stream,
	};
}

describe("Codex engine run", () => {
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
							harness_context: {
								content: "End the run after durable acceptance.",
								version: "v1",
							},
							initial_text: "Start",
							model: "gpt-5",
							permission_policy: {
								approval: "never",
								network_access: true,
								write_access: true,
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
							harness_context: {
								content: "End the run after durable acceptance.",
								version: "v1",
							},
							next_text: "Resume",
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
				).pipe(Effect.provide(make_layer({ transport_selection: "app_server_only" }))),
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
						config: { sandbox_workspace_write: { network_access: true } },
						cwd: "C:\\workspace",
						developerInstructions: [
							"Global guidance:\nUse project guidance.",
							"Artisan harness policy (v1):\nEnd the run after durable acceptance.",
						].join("\n\n"),
						model: "gpt-5",
						sandbox: "workspaceWrite",
					},
				},
				{
					method: "turn/start",
					params: {
						input: [{ text: "Start", text_elements: [], type: "text" }],
						threadId: "thread-started",
					},
				},
				{
					method: "thread/resume",
					params: {
						approvalPolicy: "on-request",
						cwd: "C:\\workspace",
						developerInstructions: [
							"Global guidance:\nUse project guidance.",
							"Artisan harness policy (v1):\nEnd the run after durable acceptance.",
						].join("\n\n"),
						sandbox: "readOnly",
						threadId: "thread-resumed",
					},
				},
				{
					method: "turn/start",
					params: {
						input: [{ text: "Resume", text_elements: [], type: "text" }],
						threadId: "thread-resumed",
					},
				},
			]);
		} finally {
			await rm(directory, { force: true, recursive: true });
		}
	});

	it("composes harness policy after editable guidance without changing user turns", async () => {
		const base = {
			_tag: "start" as const,
			artisan_run_id: "composition",
			initial_text: "Only this text belongs to the user turn.",
			working_directory: "C:\\workspace",
		};
		const harness_context = { content: "Harness policy.", version: "v1" };
		const global_guidance = {
			content: "Editable guidance.",
			source_file: "C:\\workspace\\AGENTS.md",
		};

		const [harness_only, guidance_only, both] = await Effect.runPromise(
			Effect.all([
				MakeCodexAppServerThreadOptions({ ...base, harness_context }),
				MakeCodexAppServerThreadOptions({ ...base, global_guidance }),
				MakeCodexAppServerThreadOptions({ ...base, global_guidance, harness_context }),
			]),
		);

		expect(harness_only.developerInstructions).toBe(
			"Artisan harness policy (v1):\nHarness policy.",
		);
		expect(guidance_only.developerInstructions).toBe("Global guidance:\nEditable guidance.");
		expect(both.developerInstructions).toBe(
			[
				"Global guidance:\nEditable guidance.",
				"Artisan harness policy (v1):\nHarness policy.",
			].join("\n\n"),
		);
		expect(both.developerInstructions).not.toContain(global_guidance.source_file);
		expect(base.initial_text).not.toContain(harness_context.content);
	});

	it("rejects malformed harness input before composing app-server instructions", async () => {
		const effect = Reflect.apply(MakeCodexAppServerThreadOptions, undefined, [
			{
				_tag: "start",
				artisan_run_id: "malformed-harness",
				harness_context: null,
				initial_text: "Must remain a user turn.",
				working_directory: "C:\\workspace",
			},
		]);
		const result = await Effect.runPromise(Effect.exit(effect));
		const failure = Exit.isFailure(result) ? Cause.squash(result.cause) : undefined;

		expect(failure).toMatchObject({
			_tag: "EngineConfigurationError",
			option: "harness_context",
		});
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
		{
			label: "an empty harness context",
			metadata: {
				harness_context: { content: "", version: "v1" },
			},
			option: "harness_context.content",
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
				).pipe(Effect.provide(make_layer({ transport_selection: "app_server_only" }))),
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

	it("keeps resumed usage suppressed through a queued historical turn start", async () => {
		const initialized = {
			codexHome: "C:\\fake-codex",
			platformFamily: "windows",
			platformOs: "win32",
			userAgent: "fake-codex",
		};
		const thread = {
			id: "thread-resumed",
			status: { activeFlags: [], type: "active" },
		};
		const token_usage = (input_tokens: number, output_tokens: number) => ({
			cachedInputTokens: 0,
			inputTokens: input_tokens,
			outputTokens: output_tokens,
			reasoningOutputTokens: 0,
			totalTokens: input_tokens + output_tokens,
		});
		const replay = await Effect.runPromise(
			make_transcript_sequence_replay([
				{
					args: [fixture_path, "--version"],
					chunks: [
						{
							at_ms: 1,
							chunk_base64: Buffer.from("codex-cli 0.142.5\n").toString("base64"),
							stream: "stdout",
						},
					],
					command: process.execPath,
					exit_code: 0,
					exit_signal: null,
				},
				{
					args: [fixture_path, "app-server", "--stdio"],
					chunks: [
						jsonl_chunk(5, "stdin", {
							id: 1,
							method: "initialize",
							params: {
								capabilities: {
									experimentalApi: false,
									requestAttestation: false,
								},
								clientInfo: { name: "artisan-editor", version: "0.3.0" },
							},
						}),
						jsonl_chunk(10, "stdout", { id: 1, result: initialized }),
						jsonl_chunk(15, "stdin", { method: "initialized" }),
						jsonl_chunk(16, "stdin", { id: 2, method: "account/read", params: {} }),
						jsonl_chunk(20, "stdout", {
							id: 2,
							result: {
								account: {
									email: "fake@example.com",
									planType: "plus",
									type: "chatgpt",
								},
								requiresOpenaiAuth: false,
							},
						}),
						jsonl_chunk(25, "stdin", {
							id: 3,
							method: "thread/resume",
							params: { cwd: "C:\\workspace", threadId: "thread-resumed" },
						}),
						jsonl_chunk(30, "stdout", {
							method: "thread/started",
							params: { thread },
						}),
						jsonl_chunk(31, "stdout", {
							method: "turn/started",
							params: {
								threadId: "thread-resumed",
								turn: { id: "turn-historical", status: "inProgress" },
							},
						}),
						jsonl_chunk(32, "stdout", {
							method: "thread/tokenUsage/updated",
							params: {
								threadId: "thread-resumed",
								tokenUsage: {
									last: token_usage(10_007, 5_003),
									modelContextWindow: 200_000,
									total: token_usage(10_007, 5_003),
								},
								turnId: "turn-historical",
							},
						}),
						jsonl_chunk(35, "stdout", {
							id: 3,
							result: { thread: { ...thread, turns: [] } },
						}),
						jsonl_chunk(40, "stdin", {
							id: 4,
							method: "turn/start",
							params: {
								input: [
									{
										text: "Continue",
										text_elements: [],
										type: "text",
									},
								],
								threadId: "thread-resumed",
							},
						}),
						jsonl_chunk(45, "stdout", {
							method: "turn/started",
							params: {
								threadId: "thread-resumed",
								turn: { id: "turn-current", status: "inProgress" },
							},
						}),
						jsonl_chunk(46, "stdout", {
							id: 4,
							result: { turn: { id: "turn-current" } },
						}),
						jsonl_chunk(48, "stdout", {
							method: "thread/tokenUsage/updated",
							params: {
								threadId: "thread-resumed",
								tokenUsage: {
									last: token_usage(10_007, 5_003),
									modelContextWindow: 200_000,
									total: token_usage(10_007, 5_003),
								},
								turnId: "turn-historical",
							},
						}),
						jsonl_chunk(50, "stdout", {
							method: "thread/tokenUsage/updated",
							params: {
								threadId: "thread-resumed",
								tokenUsage: {
									last: token_usage(7, 3),
									modelContextWindow: 200_000,
									total: token_usage(10_014, 5_006),
								},
								turnId: "turn-current",
							},
						}),
						jsonl_chunk(55, "stdout", {
							method: "turn/completed",
							params: {
								threadId: "thread-resumed",
								turn: { id: "turn-current", status: "completed" },
							},
						}),
					],
					command: process.execPath,
					exit_at_ms: 1_000,
					exit_code: null,
					exit_signal: "SIGTERM",
				},
			]),
		);
		const events = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;
					const run = yield* engine.Open({
						_tag: "resume",
						artisan_run_id: "run-resume-usage-baseline",
						next_text: "Continue",
						resume_token: { native_thread_id: "thread-resumed" },
						working_directory: "C:\\workspace",
					});

					return yield* run.Events.pipe(Stream.runCollect);
				}),
			).pipe(
				Effect.provide(
					make_codex_engine_layer({
						executable: process.execPath,
						executable_args: [fixture_path],
						transport_selection: "app_server_only",
					}).pipe(Layer.provide(replay.Layer)),
				),
			),
		);
		const usage = [...events].filter((event) => event._tag === "usage");
		const raw_usage = [...events].filter(
			(event) => event.raw.native_method === "thread/tokenUsage/updated",
		);

		await Effect.runPromise(replay.Assert);
		await Effect.runPromise(replay.AssertClosed);

		expect(raw_usage).toMatchObject([
			{
				_tag: "native_action",
				detail: "Resumed-thread usage retained only in raw provenance",
				raw: { frame: { turnId: "turn-historical" } },
			},
			{
				_tag: "native_action",
				detail: "Resumed-thread usage retained only in raw provenance",
				raw: { frame: { turnId: "turn-historical" } },
			},
			{
				_tag: "usage",
				raw: { frame: { turnId: "turn-current" } },
				turn_id: "turn-current",
			},
		]);
		expect(usage).toEqual([
			expect.objectContaining({
				input_tokens: 7,
				output_tokens: 3,
				turn_id: "turn-current",
			}),
		]);
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

	it("fails explicitly when the canonical event consumer falls behind", async () => {
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
					const terminal = yield* run.Closed;
					const events = yield* run.Events.pipe(Stream.runCollect);

					return { events: [...events], terminal };
				}),
			).pipe(Effect.provide(make_layer({ event_capacity: 4 }))),
		);

		expect(result.terminal).toBe("failed");
		expect(terminals(result.events)).toEqual([expect.objectContaining({ state: "failed" })]);
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
