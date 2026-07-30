import { Buffer } from "node:buffer";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it } from "vitest";
import { Cause, Deferred, Effect, Exit, Fiber, Layer, Ref, Stream } from "effect";
import { NodeFileSystem } from "@effect/platform-node-shared";
import { FileSystem } from "effect/FileSystem";
import { TestClock } from "effect/testing";

import {
	CodexEngine,
	CodexProcessFactory,
	CodexProcessFactoryLive,
	make_codex_engine_layer,
	type EngineObservation,
} from "@artisan/engines";
import { make_codex_exec_engine } from "../../modules/engines/src/codex/exec-engine";
import { MakeCodexExecEventBuffer } from "../../modules/engines/src/codex/internal/exec-event-buffer";
import { WatchCodexExecTimeout } from "../../modules/engines/src/codex/internal/exec-run";

const fixture_path = fileURLToPath(new URL("./fixtures/fake-app-server.ts", import.meta.url));
const transcript_path = fileURLToPath(
	new URL("./fixtures/transcripts/codex-exec-jsonl.jsonl", import.meta.url),
);
const original_app_scenario = process.env.FAKE_APP_SERVER_SCENARIO;
const original_exec_grandchild_pid_file = process.env.FAKE_CODEX_EXEC_GRANDCHILD_PID_FILE;
const original_exec_invocation_file = process.env.FAKE_CODEX_EXEC_INVOCATION_FILE;
const original_exec_pid_file = process.env.FAKE_CODEX_EXEC_PID_FILE;
const original_exec_scenario = process.env.FAKE_CODEX_EXEC_SCENARIO;
const original_exec_stdin_file = process.env.FAKE_CODEX_EXEC_STDIN_FILE;

afterEach(() => {
	for (const [name, value] of [
		["FAKE_APP_SERVER_SCENARIO", original_app_scenario],
		["FAKE_CODEX_EXEC_GRANDCHILD_PID_FILE", original_exec_grandchild_pid_file],
		["FAKE_CODEX_EXEC_INVOCATION_FILE", original_exec_invocation_file],
		["FAKE_CODEX_EXEC_PID_FILE", original_exec_pid_file],
		["FAKE_CODEX_EXEC_SCENARIO", original_exec_scenario],
		["FAKE_CODEX_EXEC_STDIN_FILE", original_exec_stdin_file],
	] as const) {
		if (value === undefined) {
			delete process.env[name];
		} else {
			process.env[name] = value;
		}
	}
});

function make_layer(options: Record<string, unknown> = {}) {
	return make_codex_engine_layer({
		executable: process.execPath,
		executable_args: [fixture_path],
		...options,
	}).pipe(Layer.provide(CodexProcessFactoryLive));
}

function terminals(events: ReadonlyArray<EngineObservation>) {
	return events.filter((event) => event._tag === "run_terminal");
}

function failure_from<A>(exit: Exit.Exit<A, unknown>) {
	return Exit.isFailure(exit) ? Cause.squash(exit.cause) : undefined;
}

function is_process_alive(pid: number) {
	try {
		process.kill(pid, 0);

		return true;
	} catch {
		return false;
	}
}

function make_stalling_stream() {
	let finish: (() => void) | undefined;
	const iterable: AsyncIterable<Uint8Array> = {
		[Symbol.asyncIterator]: () => ({
			next: () =>
				new Promise<IteratorResult<Uint8Array>>((resolve) => {
					finish = () => resolve({ done: true, value: undefined });
				}),
			return: async () => {
				finish?.();

				return { done: true, value: undefined };
			},
		}),
	};

	return { close: () => finish?.(), iterable };
}

function make_stalling_process_layer() {
	return Layer.succeed(CodexProcessFactory, {
		Spawn: () =>
			Effect.gen(function* () {
				const exited = yield* Deferred.make<{ code: number | null; signal: null }>();
				const stderr = make_stalling_stream();
				const stdout = make_stalling_stream();
				const Close = Effect.sync(() => {
					stderr.close();
					stdout.close();
				}).pipe(
					Effect.andThen(Deferred.succeed(exited, { code: 0, signal: null })),
					Effect.asVoid,
				);

				return {
					Close,
					Exit: Deferred.await(exited),
					Kill: () => Close,
					Stderr: stderr.iterable,
					Stdout: stdout.iterable,
					Write: () => Effect.void,
					EndInput: Effect.void,
				};
			}),
	});
}

async function collect_exec_events(scenario: string, options: Record<string, unknown> = {}) {
	process.env.FAKE_APP_SERVER_SCENARIO = "exec-fallback";
	process.env.FAKE_CODEX_EXEC_SCENARIO = scenario;

	return Effect.runPromise(
		Effect.scoped(
			Effect.gen(function* () {
				const engine = yield* CodexEngine;
				const run = yield* engine.Open({
					_tag: "start",
					artisan_run_id: `exec-${scenario}`,
					initial_text: `Run ${scenario}`,
					working_directory: "C:\\workspace",
				});
				const events = yield* run.Events.pipe(Stream.runCollect);

				return { descriptor: engine.Descriptor, events: [...events] };
			}),
		).pipe(Effect.provide(make_layer(options))),
	);
}

describe("Codex exec fallback", () => {
	it("reports a missing fallback executable as not ready", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-missing-codex-"));
		const missing_executable = join(directory, "missing-codex.exe");

		try {
			const result = await Effect.runPromise(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;

					return {
						descriptor: engine.Descriptor,
						probe: yield* engine.Probe({}).pipe(Effect.exit),
					};
				}).pipe(
					Effect.provide(
						make_layer({ executable: missing_executable, executable_args: [] }),
					),
				),
			);

			expect(result.descriptor.transport).toBe("codex-exec-jsonl");
			expect(failure_from(result.probe)).toMatchObject({
				_tag: "EngineProcessError",
			});
		} finally {
			await rm(directory, { force: true, recursive: true });
		}
	});

	it("fails a bounded version timeout and closes the stalled process", async () => {
		const layer = make_codex_engine_layer({
			executable: "fake-codex",
			version_timeout_ms: 20,
		}).pipe(Layer.provide(make_stalling_process_layer()));
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const engine = yield* CodexEngine;

				return {
					descriptor: engine.Descriptor,
					probe: yield* engine.Probe({}).pipe(Effect.exit),
				};
			}).pipe(Effect.provide(layer)),
		);

		expect(result.descriptor.transport).toBe("codex-exec-jsonl");
		expect(failure_from(result.probe)).toMatchObject({
			_tag: "EngineProbeTimeoutError",
		});
	});

	it("fails a nonzero version probe honestly", async () => {
		process.env.FAKE_CODEX_EXEC_SCENARIO = "version-nonzero";

		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const engine = yield* CodexEngine;

				return yield* engine.Probe({}).pipe(Effect.exit);
			}).pipe(Effect.provide(make_layer({ version_timeout_ms: 5_000 }))),
		);

		expect(failure_from(result)).toMatchObject({ _tag: "EngineUnavailableError" });
	});

	it("parses a fragmented fallback version", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "exec-fallback";
		process.env.FAKE_CODEX_EXEC_SCENARIO = "version-fragmented";

		const probe = await Effect.runPromise(
			Effect.gen(function* () {
				const engine = yield* CodexEngine;

				return yield* engine.Probe({});
			}).pipe(Effect.provide(make_layer())),
		);

		expect(probe.version).toBe("0.142.5");
	});

	it.each(["stdout", "stderr"] as const)(
		"bounds fallback version %s under transport selection",
		async (channel) => {
			process.env.FAKE_APP_SERVER_SCENARIO = "exec-fallback";
			process.env.FAKE_CODEX_EXEC_SCENARIO = `version-${channel}-overflow`;

			const result = await Effect.runPromise(
				Effect.gen(function* () {
					const engine = yield* CodexEngine;

					return yield* engine.Probe({}).pipe(Effect.exit);
				}).pipe(Effect.provide(make_layer())),
			);

			expect(failure_from(result)).toMatchObject({
				_tag: "EngineProtocolError",
				message: `Codex version ${channel} exceeded 65536 bytes`,
			});
		},
	);

	it.each(["thread-start-failure", "turn-start-failure"])(
		"does not downgrade after ambiguous app-server side effects: %s",
		async (scenario) => {
			const directory = await mkdtemp(join(tmpdir(), "artisan-no-exec-downgrade-"));
			const invocation_path = join(directory, "exec-invocations.jsonl");

			process.env.FAKE_APP_SERVER_SCENARIO = scenario;
			process.env.FAKE_CODEX_EXEC_INVOCATION_FILE = invocation_path;
			process.env.FAKE_CODEX_EXEC_SCENARIO = "transcript";

			try {
				const result = await Effect.runPromise(
					Effect.scoped(
						Effect.gen(function* () {
							const engine = yield* CodexEngine;
							const opened = yield* engine
								.Open({
									_tag: "start",
									artisan_run_id: `ambiguous-${scenario}`,
									initial_text: "Do not retry this",
									working_directory: "C:\\workspace",
								})
								.pipe(Effect.exit);

							return { descriptor: engine.Descriptor, opened };
						}),
					).pipe(Effect.provide(make_layer())),
				);

				expect(result.descriptor.transport).not.toBe("codex-exec-jsonl");
				expect(Exit.isFailure(result.opened)).toBe(true);
				expect(existsSync(invocation_path)).toBe(false);
			} finally {
				await rm(directory, { force: true, recursive: true });
			}
		},
		15_000,
	);

	it("cancels its run timeout after a normal terminal observation", async () => {
		const timed_out = await Effect.runPromise(
			Effect.gen(function* () {
				const closed = yield* Deferred.make<"completed">();
				const fired = yield* Ref.make(false);
				const watcher = yield* WatchCodexExecTimeout(
					Deferred.await(closed),
					60_000,
					Ref.set(fired, true),
				).pipe(Effect.forkChild);

				yield* Deferred.succeed(closed, "completed");
				yield* TestClock.adjust(60_000);
				yield* Fiber.join(watcher);

				return yield* Ref.get(fired);
			}).pipe(Effect.provide(TestClock.layer())),
		);

		expect(timed_out).toBe(false);
	});

	it.each([
		["cancel", "cancelled"],
		["timeout", "failed"],
	] as const)(
		"keeps sequences contiguous and terminal-last when emit races %s",
		async (_race, terminal_state) => {
			const result = await Effect.runPromise(
				Effect.gen(function* () {
					const allow_enqueue = yield* Deferred.make<void>();
					const emit_reserved = yield* Deferred.make<void>();
					const finish_started = yield* Deferred.make<void>();
					const close_count = yield* Ref.make(0);
					const buffer = yield* MakeCodexExecEventBuffer({
						artisan_run_id: `race-${terminal_state}`,
						BeforeEnqueue: () =>
							Deferred.succeed(emit_reserved, undefined).pipe(
								Effect.andThen(Deferred.await(allow_enqueue)),
								Effect.asVoid,
							),
						BeforeFinish: Deferred.succeed(finish_started, undefined).pipe(
							Effect.asVoid,
						),
						capacity: 4,
						CloseProcess: Ref.update(close_count, (count) => count + 1),
					});
					const events_fiber = yield* buffer.Events.pipe(
						Stream.runCollect,
						Effect.forkChild,
					);
					const emit_fiber = yield* buffer
						.Emit({
							_tag: "process_diagnostic",
							artisan_run_id: `race-${terminal_state}`,
							level: "info",
							message: "reserved before terminal",
							observation_id: `race-${terminal_state}:reserved`,
							raw: {
								engine_id: "codex",
								frame: { source: "race-test" },
								transport: "codex-exec-jsonl",
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

					return {
						close_count: yield* Ref.get(close_count),
						events: [...(yield* Fiber.join(events_fiber))],
					};
				}),
			);

			expect(result.events.map(({ sequence }) => sequence)).toEqual([1, 2]);
			expect(result.events.at(-1)).toMatchObject({
				_tag: "run_terminal",
				state: terminal_state,
			});
			expect(result.close_count).toBe(1);
		},
	);

	it.each([
		["malformed", {}, "completed", "Malformed Codex exec JSONL"],
		["oversized", { exec_max_frame_bytes: 128 }, "failed", "frame exceeded 128 bytes"],
		["stdout-overflow", { exec_max_stdout_bytes: 128 }, "failed", "stdout exceeded 128 bytes"],
		["stderr-overflow", { exec_max_stderr_bytes: 128 }, "failed", "stderr exceeded 128 bytes"],
		["nonzero", {}, "failed", "exited with code 17"],
		["hang", { exec_timeout_ms: 40 }, "failed", "timed out after 40ms"],
	] as const)(
		"handles bounded exec lifecycle scenario %s",
		async (scenario, options, terminal_state, diagnostic_text) => {
			const result = await collect_exec_events(scenario, options);
			const diagnostics = result.events.filter(
				(event) =>
					event._tag === "process_diagnostic" || event._tag === "protocol_diagnostic",
			);

			expect(terminals(result.events)).toEqual([
				expect.objectContaining({ state: terminal_state }),
			]);
			expect(diagnostics).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						message: expect.stringContaining(diagnostic_text),
					}),
				]),
			);

			if (scenario === "malformed") {
				expect(result.events).toEqual(
					expect.arrayContaining([
						expect.objectContaining({ _tag: "run_state", state: "running" }),
						expect.objectContaining({ _tag: "turn_state", state: "completed" }),
					]),
				);
			}
		},
	);

	it.each([
		["turn-failed-exit-zero", { _tag: "turn_state", state: "failed" }],
		["error-exit-zero", { _tag: "protocol_diagnostic", level: "error" }],
	] as const)(
		"lets semantic failure dominate a zero exit for %s",
		async (scenario, fatal_observation) => {
			const result = await collect_exec_events(scenario);
			const sequences = result.events.map((event) => event.sequence);
			const terminal_events = terminals(result.events);

			expect(sequences).toEqual(
				Array.from({ length: sequences.length }, (_, index) => index + 1),
			);
			expect(result.events).toEqual(
				expect.arrayContaining([expect.objectContaining(fatal_observation)]),
			);
			expect(terminal_events).toEqual([expect.objectContaining({ state: "failed" })]);
			expect(result.events.at(-1)).toEqual(terminal_events[0]);
			expect(terminal_events).not.toContainEqual(
				expect.objectContaining({ state: "completed" }),
			);
			expect(result.events).not.toContainEqual(
				expect.objectContaining({
					_tag: "process_diagnostic",
					message: expect.stringContaining("exited with code 0"),
				}),
			);
		},
	);

	it("fails boundedly under observation backpressure with exactly one terminal", async () => {
		const result = await collect_exec_events("transcript", { event_capacity: 1 });

		expect(terminals(result.events)).toEqual([expect.objectContaining({ state: "failed" })]);
	});

	it("retains fragmented unknown events and stderr byte-for-byte", async () => {
		const result = await collect_exec_events("transcript");
		const transcript_lines = (await readFile(transcript_path, "utf8")).trim().split("\n");
		const unknown_line = transcript_lines.find((line) => line.includes('"future.event"'))!;
		const unknown = result.events.find(
			(event) => event._tag === "native_action" && event.action === "future.event",
		);
		const stderr = result.events.find(
			(event) =>
				event._tag === "process_diagnostic" &&
				event.message.includes("sanitized exec diagnostic"),
		);

		expect(unknown).toBeDefined();
		expect(Buffer.from(unknown!.raw.raw_frame_base64!, "base64").toString("utf8")).toBe(
			unknown_line,
		);
		expect(stderr).toBeDefined();
		expect(Buffer.from(stderr!.raw.raw_frame_base64!, "base64").toString("utf8")).toBe(
			"sanitized exec diagnostic\n",
		);
		expect(result.events).toEqual(
			expect.arrayContaining([
				expect.objectContaining({ _tag: "reasoning_summary_completed" }),
				expect.objectContaining({ _tag: "terminal_activity" }),
				expect.objectContaining({ _tag: "file" }),
				expect.objectContaining({ _tag: "search" }),
				expect.objectContaining({ _tag: "tool" }),
				expect.objectContaining({ _tag: "plan" }),
			]),
		);
		expect(terminals(result.events)).toHaveLength(1);
	});

	it("rejects resume before spawning a paid exec run", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-exec-resume-"));
		const invocation_path = join(directory, "exec-invocations.jsonl");

		process.env.FAKE_APP_SERVER_SCENARIO = "exec-fallback";
		process.env.FAKE_CODEX_EXEC_INVOCATION_FILE = invocation_path;
		process.env.FAKE_CODEX_EXEC_SCENARIO = "transcript";

		try {
			const result = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const engine = yield* CodexEngine;

						return yield* engine
							.Open({
								_tag: "resume",
								artisan_run_id: "exec-resume",
								resume_token: { native_thread_id: "native-thread" },
								working_directory: "C:\\workspace",
							})
							.pipe(Effect.exit);
					}),
				).pipe(Effect.provide(make_layer())),
			);

			expect(failure_from(result)).toMatchObject({
				_tag: "EngineUnsupportedOperationError",
				operation: "resume",
			});
			expect(existsSync(invocation_path)).toBe(false);
		} finally {
			await rm(directory, { force: true, recursive: true });
		}
	});

	it.each([
		["cancel", "hang", "cancelled"],
		["close", "hang-ignore-term", "closed"],
	] as const)(
		"supports %s with escalation, exact-one terminal, and orphan cleanup",
		async (command_tag, scenario, terminal_state) => {
			const directory = await mkdtemp(join(tmpdir(), `artisan-exec-${command_tag}-`));
			const pid_path = join(directory, "exec.pid");
			const grandchild_pid_path = join(directory, "grandchild.pid");

			process.env.FAKE_APP_SERVER_SCENARIO = "exec-fallback";
			process.env.FAKE_CODEX_EXEC_GRANDCHILD_PID_FILE = grandchild_pid_path;
			process.env.FAKE_CODEX_EXEC_PID_FILE = pid_path;
			process.env.FAKE_CODEX_EXEC_SCENARIO = scenario;

			try {
				const result = await Effect.runPromise(
					Effect.scoped(
						Effect.gen(function* () {
							const engine = yield* CodexEngine;
							const ready = yield* Deferred.make<void>();
							const run = yield* engine.Open({
								_tag: "start",
								artisan_run_id: `exec-${command_tag}`,
								initial_text: `Wait for ${command_tag}`,
								working_directory: "C:\\workspace",
							});
							const events_fiber = yield* run.Events.pipe(
								Stream.tap((event) =>
									event._tag === "native_action" &&
									event.action === "fixture.ready"
										? Deferred.succeed(ready, undefined).pipe(Effect.ignore)
										: Effect.void,
								),
								Stream.runCollect,
								Effect.forkChild,
							);

							yield* Deferred.await(ready);

							const unsupported =
								command_tag === "cancel"
									? yield* Effect.all([
											run
												.Send({
													_tag: "steer",
													command_id: "unsupported-steer",
													text: "Steer",
												})
												.pipe(Effect.exit),
											run
												.Send({
													_tag: "respond_approval",
													approval_id: "approval",
													approved: true,
													command_id: "unsupported-approval",
												})
												.pipe(Effect.exit),
											run
												.Send({
													_tag: "respond_question",
													answers: { question: ["answer"] },
													command_id: "unsupported-question",
												})
												.pipe(Effect.exit),
										])
									: [];
							const command =
								command_tag === "cancel"
									? ({
											_tag: "cancel",
											command_id: `finish-${command_tag}`,
										} as const)
									: ({
											_tag: "close",
											command_id: `finish-${command_tag}`,
										} as const);
							const pid = Number(
								yield* Effect.promise(() => readFile(pid_path, "utf8")),
							);
							const grandchild_pid = Number(
								yield* Effect.promise(() => readFile(grandchild_pid_path, "utf8")),
							);

							yield* run.Send(command);
							yield* run.Send(command);

							return {
								events: [...(yield* Fiber.join(events_fiber))],
								grandchild_pid,
								pid,
								unsupported,
							};
						}),
					).pipe(Effect.provide(make_layer())),
				);

				expect(terminals(result.events)).toEqual([
					expect.objectContaining({ state: terminal_state }),
				]);
				expect(is_process_alive(result.pid)).toBe(false);
				expect(is_process_alive(result.grandchild_pid)).toBe(false);
				for (const unsupported of result.unsupported) {
					expect(failure_from(unsupported)).toMatchObject({
						_tag: "EngineUnsupportedCommandError",
					});
				}
			} finally {
				await rm(directory, { force: true, recursive: true });
			}
		},
	);

	it("selects a truthful one-shot Engine before opening a run", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-codex-exec-"));
		const invocation_path = join(directory, "invocations.jsonl");
		const stdin_path = join(directory, "stdin.txt");

		process.env.FAKE_APP_SERVER_SCENARIO = "exec-fallback";
		process.env.FAKE_CODEX_EXEC_INVOCATION_FILE = invocation_path;
		process.env.FAKE_CODEX_EXEC_SCENARIO = "transcript";
		process.env.FAKE_CODEX_EXEC_STDIN_FILE = stdin_path;

		try {
			const result = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const engine = yield* CodexEngine;
						const run = yield* engine.Open({
							_tag: "start",
							artisan_run_id: "exec-fallback-run",
							initial_text: "Use the one-shot fallback",
							model: "gpt-5",
							permission_policy: {
								approval: "on_request",
								network_access: false,
								write_access: true,
							},
							provider_options: {
								"codex.exec.profile": "fixture-profile",
								"codex.exec.skip_git_repo_check": true,
								"codex.reasoning_effort": "high",
							},
							working_directory: "C:\\workspace",
						});
						const events = yield* run.Events.pipe(Stream.runCollect);

						return { descriptor: engine.Descriptor, events: [...events] };
					}),
				).pipe(Effect.provide(make_layer())),
			);
			const invocations = (await readFile(invocation_path, "utf8"))
				.trim()
				.split("\n")
				.map((line) => JSON.parse(line) as ReadonlyArray<string>);
			const stdin = await readFile(stdin_path, "utf8");

			expect(result.descriptor).toMatchObject({
				capabilities: {
					approval: { state: "unsupported" },
					cancel: { state: "supported" },
					global_guidance: { state: "unsupported" },
					resume: { state: "unsupported" },
					steer: { state: "unsupported" },
				},
				id: "codex",
				transport: "codex-exec-jsonl",
			});
			expect(invocations).toEqual([
				[
					"exec",
					"--json",
					"--color",
					"never",
					"--cd",
					"C:\\workspace",
					"--model",
					"gpt-5",
					"-c",
					'approval_policy="on-request"',
					"-c",
					"sandbox_workspace_write.network_access=false",
					"-c",
					'model_reasoning_effort="high"',
					"--profile",
					"fixture-profile",
					"--sandbox",
					"workspace-write",
					"--skip-git-repo-check",
					"-",
				],
			]);
			expect(stdin).toBe("Use the one-shot fallback");
			expect(result.events).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						_tag: "process_diagnostic",
						level: "warning",
						raw: expect.objectContaining({
							frame: expect.objectContaining({
								reason: expect.stringMatching(/\S/),
								source: "startup-transport-selection",
							}),
						}),
					}),
					expect.objectContaining({
						_tag: "agent_message_completed",
						message: "Fallback complete.",
					}),
				]),
			);
			expect(terminals(result.events)).toEqual([
				expect.objectContaining({ state: "completed" }),
			]);
		} finally {
			await rm(directory, { force: true, recursive: true });
		}
	});

	it("rejects runtime global guidance before any exec provider side effect", async () => {
		let spawn_count = 0;
		const file_system = await Effect.runPromise(
			FileSystem.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const engine = make_codex_exec_engine({
			event_capacity: 16,
			executable: "codex",
			executable_args: [],
			fallback_reason: "test fallback",
			file_system,
			factory: {
				Spawn: () => {
					spawn_count += 1;
					return Effect.die("spawned");
				},
			},
			max_frame_bytes: 1_024,
			max_stderr_bytes: 1_024,
			max_stdout_bytes: 1_024,
			timeout_ms: 1_000,
			version_timeout_ms: 1_000,
		});
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.exit(
					engine.Open({
						_tag: "start",
						artisan_run_id: "exec-guidance-reject",
						global_guidance: {
							content: "Do not pass this to argv.",
							source_file: "C:\\workspace\\AGENTS.md",
						},
						initial_text: "User text stays separate.",
						working_directory: "C:\\workspace",
					}),
				),
			),
		);

		expect(Exit.isFailure(result)).toBe(true);
		expect(failure_from(result)).toMatchObject({
			_tag: "EngineUnsupportedOperationError",
			operation: "global_guidance",
		});
		expect(spawn_count).toBe(0);
	});
});
