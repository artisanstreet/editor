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

const fixture_path = fileURLToPath(new URL("./fixtures/fake-app-server.mjs", import.meta.url));
const original_pid_file = process.env.FAKE_APP_SERVER_PID_FILE;
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
});

function make_layer(options: { readonly event_capacity?: number } = {}) {
	return make_codex_engine_layer({
		...options,
		executable: process.execPath,
		executable_args: [fixture_path],
		shell: false,
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
						permission_profile: ":workspace",
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

	it("ignores stale turn completion when a newer native turn is active", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "stale-turn";

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;
					const run = yield* engine.Open({
						_tag: "start",
						artisan_run_id: "run-stale-turn",
						initial_text: "Start",
						working_directory: "C:\\workspace",
					});

					yield* Effect.sleep(25);

					const steer = yield* run
						.Send({ _tag: "steer", command_id: "steer-newer", text: "Continue" })
						.pipe(Effect.exit);

					yield* run.Send({ _tag: "close", command_id: "close-stale-test" });

					return { events: yield* run.Events.pipe(Stream.runCollect), steer };
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
