import { createHash } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeCrypto } from "@effect/platform-node-shared";
import { Effect, Layer, ManagedRuntime, Option } from "effect";
import { describe, expect, it } from "vitest";

import type { EngineObservation } from "@artisan/engines";

import {
	encode_portable_checkpoint_content,
	type PortableCheckpoint,
} from "../../modules/backend/src/orchestration/thread-continuation-model";
import { make_database_layer, Database } from "../../modules/backend/src/persistence/database";
import {
	JournalEvents,
	OrchestrationCoordinators,
	OrchestrationMessages,
	OrchestrationOutbox,
	OrchestrationRawObservations,
	OrchestrationRuns,
	ThreadErasureClaims,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import {
	ThreadContinuationConflict,
	ThreadContinuationRepository,
	ThreadContinuationRepositoryLive,
	ThreadContinuationFailure,
} from "../../modules/backend/src/persistence/thread-continuation-repository";
import {
	ThreadContinuationLaunches,
	ThreadPortableHandoffs,
	ThreadRunContinuationState,
} from "../../modules/backend/src/persistence/thread-continuation-schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const now = "2026-07-30T10:00:00.000Z";

const make_runtime = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-continuation-"));
	const database = make_database_layer({
		database_path: join(directory, "artisan.db"),
		migrations_path,
	});
	const metadata = Layer.succeed(
		RuntimeMetadata,
		RuntimeMetadata.of({
			instance_id: "backend-test",
			MakeId: (prefix) => Effect.succeed(`${prefix}-test`),
			Now: Effect.succeed(now),
		}),
	);
	const base = Layer.mergeAll(database, metadata, NodeCrypto.layer);
	const runtime = ManagedRuntime.make(
		Layer.mergeAll(base, ThreadContinuationRepositoryLive.pipe(Layer.provide(base))),
	);
	return { directory, runtime };
};

type TestRuntime = Awaited<ReturnType<typeof make_runtime>>["runtime"];

const with_runtime = async (run: (runtime: TestRuntime) => Promise<void>) => {
	const { directory, runtime } = await make_runtime();
	try {
		await run(runtime);
	} finally {
		await runtime.dispose();
		await rm(directory, { force: true, recursive: true });
	}
};

const database_from = (runtime: TestRuntime) =>
	runtime.runPromise(
		Effect.gen(function* () {
			return yield* Database;
		}),
	);

const repository_from = (runtime: TestRuntime) =>
	runtime.runPromise(
		Effect.gen(function* () {
			return yield* ThreadContinuationRepository;
		}),
	);

type SeedOptions = {
	readonly include_target_event?: boolean;
	readonly source_engine_id?: string;
	readonly source_model_id?: string | null;
	readonly source_native_resume_json?: string | null;
	readonly source_native_thread_id?: string | null;
	readonly source_status?: string;
	readonly target_engine_id?: string;
	readonly target_model_id?: string | null;
};

const Seed = (options: SeedOptions = {}) =>
	Effect.gen(function* () {
		const database = yield* Database;
		const source_engine_id = options.source_engine_id ?? "engine-a";
		const target_engine_id = options.target_engine_id ?? source_engine_id;
		yield* database.client.insert(Threads).values({
			created_at: now,
			thread_id: "thread-1",
			title: "Thread",
			updated_at: now,
		});
		yield* database.client.insert(OrchestrationCoordinators).values({
			agent_id: "agent-1",
			created_at: now,
			display_name: "Agent",
			engine_id: target_engine_id,
			role: "coordinator",
			thread_id: "thread-1",
			updated_at: now,
		});
		yield* database.client.insert(OrchestrationRuns).values([
			{
				agent_id: "agent-1",
				created_at: now,
				engine_id: source_engine_id,
				model_id: options.source_model_id ?? null,
				native_resume_json: options.source_native_resume_json ?? null,
				native_thread_id: options.source_native_thread_id ?? null,
				run_id: "run-1",
				status: options.source_status ?? "completed",
				thread_id: "thread-1",
				updated_at: now,
				working_directory: "C:/work",
			},
			{
				agent_id: "agent-1",
				created_at: now,
				engine_id: target_engine_id,
				model_id: options.target_model_id ?? null,
				run_id: "run-2",
				status: "queued",
				thread_id: "thread-1",
				updated_at: now,
				working_directory: "C:/work",
			},
		]);
		yield* database.client.insert(OrchestrationMessages).values({
			agent_id: "agent-1",
			command_id: "command-2",
			created_at: now,
			delivery: "start",
			message_id: "message-2",
			run_id: "run-2",
			text: "next",
			thread_id: "thread-1",
		});
		yield* database.client.insert(OrchestrationOutbox).values({
			agent_id: "agent-1",
			command_id: "command-2",
			created_at: now,
			kind: "start",
			payload_json: "{}",
			run_id: "run-2",
			status: "dispatching",
			thread_id: "thread-1",
			updated_at: now,
		});
		yield* InsertJournalEvent("event-1", "run-1", 1);
		if (options.include_target_event !== false)
			yield* InsertJournalEvent("event-2", "run-2", 2);
	});

const InsertJournalEvent = (event_id: string, run_id: string, stream_sequence: number) =>
	Effect.gen(function* () {
		const database = yield* Database;
		yield* database.client.insert(JournalEvents).values({
			causation_id: event_id,
			correlation_id: event_id,
			event_id,
			event_type: "test",
			occurred_at: now,
			origin: "test",
			payload_json: "{}",
			run_id,
			schema_version: 1,
			stream_id: "thread-1",
			stream_sequence,
			thread_id: "thread-1",
		});
	});

const InsertCanonicalJournalEvent = (
	event_id: string,
	run_id: string,
	stream_sequence: number,
	payload:
		| {
				readonly message_id: string;
				readonly text: string;
				readonly type: "assistant.message_completed";
		  }
		| {
				readonly message_id: string;
				readonly text: string;
				readonly type: "thread.message_queued" | "thread.message_steering";
				readonly working_directory: string;
		  },
) =>
	Effect.gen(function* () {
		const database = yield* Database;
		yield* database.client.insert(JournalEvents).values({
			causation_id: event_id,
			correlation_id: event_id,
			event_id,
			event_type: payload.type,
			occurred_at: now,
			origin: "test",
			payload_json: JSON.stringify(payload),
			run_id,
			schema_version: 1,
			stream_id: "thread-1",
			stream_sequence,
			thread_id: "thread-1",
		});
	});

const make_checkpoint = (
	overrides: Partial<Omit<PortableCheckpoint, "sha256">> = {},
): PortableCheckpoint => {
	const without_hash = {
		created_at: now,
		method: "canonical_transcript_summary" as const,
		omitted_entries: 1,
		schema_version: 1 as const,
		source: {
			cut: {
				thread_id: "thread-1",
				through_journal_sequence: 1,
				through_observation_sequence: 7,
				through_run_id: "run-1",
			},
			engine_id: "engine-a",
			model_id: "source-model",
		},
		summary: "Canonical summary",
		tail: [{ role: "assistant" as const, text: "Last settled answer" }],
		...overrides,
	};
	return {
		...without_hash,
		sha256: createHash("sha256")
			.update(encode_portable_checkpoint_content(without_hash))
			.digest("hex"),
	};
};

const expect_failure_code = async (effect: Promise<unknown>, code: string) => {
	await expect(effect).rejects.toEqual(expect.objectContaining({ code }));
};

describe("thread continuation repository", () => {
	it("uses the immediate journal predecessor when three runs are queued", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(Seed({ source_status: "queued" }));
			const database = await database_from(runtime);
			await runtime.runPromise(
				Effect.gen(function* () {
					yield* database.client.run(
						"UPDATE orchestration_runs SET status = 'completed' WHERE run_id = 'run-2'",
					);
					yield* database.client.insert(OrchestrationRuns).values({
						agent_id: "agent-1",
						created_at: now,
						engine_id: "engine-a",
						run_id: "run-3",
						status: "queued",
						thread_id: "thread-1",
						updated_at: now,
						working_directory: "C:/work",
					});
					yield* InsertJournalEvent("event-3", "run-3", 3);
				}),
			);
			const repository = await repository_from(runtime);
			expect(await runtime.runPromise(repository.IsDispatchReady("run-3"))).toBe(true);

			await runtime.runPromise(
				database.client.run(
					"UPDATE orchestration_runs SET status = 'queued' WHERE run_id = 'run-2'",
				),
			);
			expect(await runtime.runPromise(repository.IsDispatchReady("run-3"))).toBe(false);
		}));

	it("retains a settled source when its provider resume token is malformed", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(
				Seed({
					source_native_resume_json: "{not-json",
					source_native_thread_id: "source-native",
				}),
			);
			const repository = await repository_from(runtime);
			const context = await runtime.runPromise(repository.ReadContext("run-2"));
			expect(Option.isSome(context.source)).toBe(true);
			if (Option.isSome(context.source)) {
				expect(context.source.value.run_id).toBe("run-1");
				expect(Option.isNone(context.source.value.resume_token)).toBe(true);
			}
			expect(context.source_cut_journal_sequence).toBe(1);
		}));

	it("verifies hash, raw boundary, and native session before storing a private compaction", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(
				Seed({
					include_target_event: false,
					source_engine_id: "claude",
					source_native_resume_json: JSON.stringify({
						native_thread_id: "source-native",
					}),
					source_native_thread_id: "source-native",
				}),
			);
			const database = await database_from(runtime);
			const repository = await repository_from(runtime);
			const frame = {
				compactMetadata: {
					providerField: "preserved",
					trigger: "auto" as const,
				},
				providerTopLevelField: { version: 2 },
				subtype: "compact_boundary" as const,
				type: "system" as const,
				uuid: "boundary-1",
			};
			await runtime.runPromise(
				database.client.insert(OrchestrationRawObservations).values({
					engine_id: "claude",
					frame_json: JSON.stringify(frame),
					native_id: "boundary-1",
					native_method: "system.compact_boundary",
					observation_id: "observation-1",
					run_id: "run-1",
					sequence: 1,
					transport: "ndjson",
				}),
			);
			await runtime.runPromise(
				Effect.gen(function* () {
					yield* database.client.insert(OrchestrationRuns).values({
						agent_id: "agent-other",
						created_at: now,
						engine_id: "claude",
						native_thread_id: "source-native",
						run_id: "run-other",
						status: "completed",
						thread_id: "thread-1",
						updated_at: now,
						working_directory: "C:/work",
					});
					yield* InsertJournalEvent("event-other", "run-other", 2);
					yield* InsertJournalEvent("event-2", "run-2", 3);
					yield* InsertJournalEvent("event-1-late", "run-1", 4);
				}),
			);
			const observation = {
				_tag: "compaction",
				artisan_run_id: "run-1",
				observation_id: "observation-1",
				raw: {
					engine_id: "claude",
					frame,
					native_id: "boundary-1",
					native_method: "system.compact_boundary",
					transport: "ndjson",
				},
				sequence: 1,
				state: "completed",
			} satisfies EngineObservation;
			await runtime.runPromise(repository.RecordObservationMetadata(observation));

			const summary = "Provider summary\r\nwith normalized newlines";
			const summary_sha256 = createHash("sha256")
				.update(summary.replaceAll("\r\n", "\n"))
				.digest("hex");
			const valid = {
				boundary_id: "boundary-1",
				method: "claude_post_compact" as const,
				observation_id: "observation-1",
				source_native_thread_id: "source-native",
				summary,
				summary_sha256,
				trigger: "auto" as const,
			};
			await expect_failure_code(
				runtime.runPromise(
					repository.RecordNativeCompaction("run-1", {
						...valid,
						summary_sha256: "0".repeat(64),
					}),
				),
				"native_compaction_hash_mismatch",
			);
			await runtime.runPromise(
				database.client.run(
					"UPDATE orchestration_raw_observations SET native_method = 'wrong-method' WHERE observation_id = 'observation-1'",
				),
			);
			await expect_failure_code(
				runtime.runPromise(repository.RecordNativeCompaction("run-1", valid)),
				"native_compaction_raw_mismatch",
			);
			await runtime.runPromise(
				database.client.run(
					"UPDATE orchestration_raw_observations SET native_method = 'system.compact_boundary' WHERE observation_id = 'observation-1'",
				),
			);
			await expect_failure_code(
				runtime.runPromise(
					repository.RecordNativeCompaction("run-1", {
						...valid,
						source_native_thread_id: "wrong-native-session",
					}),
				),
				"native_compaction_unverified",
			);

			await runtime.runPromise(repository.RecordNativeCompaction("run-1", valid));
			await runtime.runPromise(
				database.client.insert(ThreadRunContinuationState).values({
					created_at: now,
					engine_id: "claude",
					last_observation_sequence: 1,
					native_compaction_boundary_journal_sequence: 2,
					native_compaction_json: JSON.stringify({
						...valid,
						boundary_id: "other-boundary",
						observation_id: "other-observation",
					}),
					native_compaction_observation_id: "other-observation",
					run_id: "run-other",
					thread_id: "thread-1",
					updated_at: now,
				}),
			);
			const state = (
				await runtime.runPromise(database.client.select().from(ThreadRunContinuationState))
			).find((candidate) => candidate.run_id === "run-1");
			expect(JSON.parse(state!.native_compaction_json!)).toEqual(valid);
			expect(JSON.parse(frame ? JSON.stringify(frame) : "{}")).not.toHaveProperty("summary");
			const context = await runtime.runPromise(repository.ReadContext("run-2"));
			expect(Option.isSome(context.native_compaction)).toBe(true);
			await runtime.runPromise(
				database.client.update(ThreadRunContinuationState).set({
					native_compaction_json: JSON.stringify({
						...valid,
						summary: "tampered persisted summary",
					}),
				}),
			);
			const tampered_context = await runtime.runPromise(repository.ReadContext("run-2"));
			expect(Option.isNone(tampered_context.native_compaction)).toBe(true);
			await runtime.runPromise(
				database.client
					.update(ThreadRunContinuationState)
					.set({ native_compaction_json: JSON.stringify(valid) }),
			);

			await runtime.runPromise(
				Effect.gen(function* () {
					yield* database.client.run(
						"UPDATE orchestration_runs SET status = 'completed', native_thread_id = 'source-native', native_resume_json = '{\"native_thread_id\":\"source-native\"}' WHERE run_id = 'run-2'",
					);
					yield* database.client.insert(ThreadRunContinuationState).values({
						created_at: now,
						engine_id: "claude",
						last_observation_sequence: 2,
						run_id: "run-2",
						thread_id: "thread-1",
						updated_at: now,
					});
					yield* database.client.insert(OrchestrationRuns).values({
						agent_id: "agent-1",
						created_at: now,
						engine_id: "codex",
						run_id: "run-3",
						status: "queued",
						thread_id: "thread-1",
						updated_at: now,
						working_directory: "C:/work",
					});
					yield* database.client.insert(OrchestrationMessages).values({
						agent_id: "agent-1",
						command_id: "command-3",
						created_at: now,
						delivery: "start",
						message_id: "message-3",
						run_id: "run-3",
						text: "switch after another turn",
						thread_id: "thread-1",
					});
					yield* database.client.insert(OrchestrationOutbox).values({
						agent_id: "agent-1",
						command_id: "command-3",
						created_at: now,
						kind: "start",
						payload_json: "{}",
						run_id: "run-3",
						status: "dispatching",
						thread_id: "thread-1",
						updated_at: now,
					});
					yield* InsertJournalEvent("event-3", "run-3", 5);
				}),
			);
			const later_context = await runtime.runPromise(repository.ReadContext("run-3"));
			expect(Option.getOrThrow(later_context.source).run_id).toBe("run-2");
			expect(Option.getOrThrow(later_context.native_compaction).value).toMatchObject({
				boundary_id: "boundary-1",
				summary,
			});
			const cross_engine_checkpoint = make_checkpoint({
				method: "claude_post_compact",
				source: {
					cut: {
						thread_id: "thread-1",
						through_journal_sequence: 3,
						through_observation_sequence: 2,
						through_run_id: "run-2",
					},
					engine_id: "claude",
				},
				summary,
				tail: [{ role: "assistant", text: "run-2 tail" }],
			});
			const claude_lineage = {
				boundary_id: "boundary-1",
				kind: "claude" as const,
				observation_id: "observation-1",
				source_native_thread_id: "source-native",
				through_run_id: "run-1",
			};
			await expect_failure_code(
				runtime.runPromise(
					repository.PrepareLaunch("run-3", {
						_tag: "portable",
						checkpoint: make_checkpoint({
							method: "claude_post_compact",
							source: cross_engine_checkpoint.source,
							summary: "tampered checkpoint summary",
							tail: cross_engine_checkpoint.tail,
						}),
						handoff_id: "handoff-tampered",
						lineage: claude_lineage,
						request_id: "command-3",
						source_run_id: "run-2",
					}),
				),
				"portable_lineage_mismatch",
			);
			expect(
				await runtime.runPromise(
					repository.PrepareLaunch("run-3", {
						_tag: "portable",
						checkpoint: cross_engine_checkpoint,
						handoff_id: "handoff-cross-engine",
						lineage: claude_lineage,
						request_id: "command-3",
						source_run_id: "run-2",
					}),
				),
			).toBe("prepared");

			await runtime.runPromise(
				database.client.run(
					"UPDATE orchestration_runs SET native_thread_id = 'different-session' WHERE run_id = 'run-2'",
				),
			);
			const different_session = await runtime.runPromise(repository.ReadContext("run-3"));
			expect(Option.isNone(different_session.native_compaction)).toBe(true);
		}));

	it("persists the exact portable checkpoint and makes replay idempotent but conflicting replay fail", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(
				Seed({
					source_model_id: "source-model",
					target_engine_id: "engine-b",
					target_model_id: "target-model",
				}),
			);
			const database = await database_from(runtime);
			await runtime.runPromise(
				database.client.insert(ThreadRunContinuationState).values({
					created_at: now,
					engine_id: "engine-a",
					last_observation_sequence: 7,
					model_id: "source-model",
					run_id: "run-1",
					thread_id: "thread-1",
					updated_at: now,
				}),
			);
			const repository = await repository_from(runtime);
			const checkpoint = make_checkpoint();
			const launch = {
				_tag: "portable" as const,
				checkpoint,
				handoff_id: "handoff-1",
				lineage: { kind: "canonical" as const },
				request_id: "command-2",
				source_run_id: "run-1",
				target_model_id: "target-model",
			};
			expect(await runtime.runPromise(repository.PrepareLaunch("run-2", launch))).toBe(
				"prepared",
			);
			expect(await runtime.runPromise(repository.PrepareLaunch("run-2", launch))).toBe(
				"prepared",
			);
			const handoff = (
				await runtime.runPromise(database.client.select().from(ThreadPortableHandoffs))
			).find((candidate) => candidate.target_run_id === "run-2");
			expect(handoff).toMatchObject({
				content_sha256: checkpoint.sha256,
				omitted_entries: checkpoint.omitted_entries,
				provider_lineage_json: JSON.stringify(launch.lineage),
				summary: checkpoint.summary,
				tail_json: JSON.stringify(checkpoint.tail),
				through_journal_sequence: 1,
				through_observation_sequence: 7,
			});

			const changed_checkpoint = make_checkpoint({ summary: "Different valid summary" });
			await expect(
				runtime.runPromise(
					repository.PrepareLaunch("run-2", {
						...launch,
						checkpoint: changed_checkpoint,
					}),
				),
			).rejects.toBeInstanceOf(ThreadContinuationConflict);
		}));

	it("opens exactly once and can idempotently fail an opening launch", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(
				Seed({
					source_native_resume_json: JSON.stringify({
						native_thread_id: "source-native",
					}),
					source_native_thread_id: "source-native",
				}),
			);
			const repository = await repository_from(runtime);
			await runtime.runPromise(
				repository.PrepareLaunch("run-2", {
					_tag: "native",
					request_id: "command-2",
					source_run_id: "run-1",
				}),
			);
			await runtime.runPromise(repository.MarkOpening("run-2"));
			await expect_failure_code(
				runtime.runPromise(repository.MarkOpening("run-2")),
				"launch_not_prepared",
			);
			await runtime.runPromise(repository.FailLaunch("run-2", "engine_open_failed"));
			await runtime.runPromise(repository.FailLaunch("run-2", "engine_open_failed"));
			await expect_failure_code(
				runtime.runPromise(repository.FailLaunch("run-2", "different_failure")),
				"launch_not_failable",
			);
		}));

	it("binds atomically, supports identical replay, and rejects conflicting replay", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(
				Seed({
					source_native_resume_json: JSON.stringify({
						native_thread_id: "source-native",
					}),
					source_native_thread_id: "source-native",
				}),
			);
			const database = await database_from(runtime);
			const repository = await repository_from(runtime);
			await runtime.runPromise(
				repository.PrepareLaunch("run-2", {
					_tag: "native",
					request_id: "command-2",
					source_run_id: "run-1",
					target_model_id: "target-model",
				}),
			);
			await runtime.runPromise(repository.MarkOpening("run-2"));
			await runtime.runPromise(
				Effect.gen(function* () {
					yield* database.client.run(
						"UPDATE orchestration_runs SET status = 'running' WHERE run_id = 'run-2'",
					);
					yield* database.client.run(
						"UPDATE orchestration_coordinators SET active_run_id = 'run-2' WHERE thread_id = 'thread-1'",
					);
				}),
			);

			await expect_failure_code(
				runtime.runPromise(
					repository.BindTarget({
						command_id: "command-2",
						model_id: "target-model",
						native_thread_id: "target-native",
						resume_token: { native_thread_id: "different-native" },
						target_run_id: "run-2",
					}),
				),
				"resume_token_thread_mismatch",
			);
			const before_launch = (
				await runtime.runPromise(database.client.select().from(ThreadContinuationLaunches))
			).find((candidate) => candidate.target_run_id === "run-2");
			const before_run = (
				await runtime.runPromise(database.client.select().from(OrchestrationRuns))
			).find((candidate) => candidate.run_id === "run-2");
			const before_outbox = (
				await runtime.runPromise(database.client.select().from(OrchestrationOutbox))
			).find((candidate) => candidate.command_id === "command-2");
			expect(before_launch!.state).toBe("opening");
			expect(before_run!.native_thread_id).toBeNull();
			expect(before_outbox!.status).toBe("dispatching");

			const binding = {
				command_id: "command-2",
				model_id: "target-model",
				native_thread_id: "target-native",
				resume_token: {
					native_thread_id: "target-native",
					opaque_checkpoint: "opaque",
				},
				target_run_id: "run-2",
			} as const;
			await runtime.runPromise(repository.BindTarget(binding));
			await runtime.runPromise(repository.BindTarget(binding));
			await expect(
				runtime.runPromise(
					repository.BindTarget({
						...binding,
						native_thread_id: "conflicting-native",
						resume_token: { native_thread_id: "conflicting-native" },
					}),
				),
			).rejects.toBeInstanceOf(ThreadContinuationConflict);

			const after_launch = (
				await runtime.runPromise(database.client.select().from(ThreadContinuationLaunches))
			).find((candidate) => candidate.target_run_id === "run-2");
			const after_run = (
				await runtime.runPromise(database.client.select().from(OrchestrationRuns))
			).find((candidate) => candidate.run_id === "run-2");
			const after_outbox = (
				await runtime.runPromise(database.client.select().from(OrchestrationOutbox))
			).find((candidate) => candidate.command_id === "command-2");
			expect(after_launch!.state).toBe("bound");
			expect(after_run).toMatchObject({
				model_id: "target-model",
				native_thread_id: "target-native",
				native_resume_json: JSON.stringify(binding.resume_token),
			});
			expect(after_outbox!.status).toBe("delivered");
		}));

	it("reconciles stranded launches while leaving erased threads untouched", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(
				Seed({
					source_native_resume_json: JSON.stringify({
						native_thread_id: "source-native",
					}),
					source_native_thread_id: "source-native",
				}),
			);
			const database = await database_from(runtime);
			const repository = await repository_from(runtime);
			await runtime.runPromise(
				repository.PrepareLaunch("run-2", {
					_tag: "native",
					request_id: "command-2",
					source_run_id: "run-1",
				}),
			);
			await runtime.runPromise(repository.MarkOpening("run-2"));
			await runtime.runPromise(
				database.client
					.insert(ThreadErasureClaims)
					.values({ claimed_at: now, thread_id: "thread-1" }),
			);
			expect(await runtime.runPromise(repository.ReconcileStranded())).toEqual([]);

			await runtime.runPromise(
				database.client.run(
					"DELETE FROM thread_erasure_claims WHERE thread_id = 'thread-1'",
				),
			);
			expect(await runtime.runPromise(repository.ReconcileStranded())).toEqual(["run-2"]);
			const launch = (
				await runtime.runPromise(database.client.select().from(ThreadContinuationLaunches))
			).find((candidate) => candidate.target_run_id === "run-2");
			expect(launch).toMatchObject({
				failure_code: "stranded_recovery",
				state: "failed",
			});
		}));

	it("fences context reads after an erasure claim", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(Seed());
			const database = await database_from(runtime);
			await runtime.runPromise(
				database.client
					.insert(ThreadErasureClaims)
					.values({ claimed_at: now, thread_id: "thread-1" }),
			);
			const repository = await repository_from(runtime);
			await expect_failure_code(
				runtime.runPromise(repository.ReadContext("run-2")),
				"thread_unavailable",
			);
		}));

	it("reads later logical runs after a same-run compaction boundary despite journal interleaving", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(Seed());
			const database = await database_from(runtime);
			await runtime.runPromise(
				Effect.gen(function* () {
					yield* database.client.run("DELETE FROM journal_events");
					yield* InsertCanonicalJournalEvent("run-1-user", "run-1", 1, {
						message_id: "message-run-1",
						text: "first objective",
						type: "thread.message_queued",
						working_directory: "C:/work",
					});
					yield* InsertCanonicalJournalEvent("run-2-user", "run-2", 2, {
						message_id: "message-run-2",
						text: "queued follow-up",
						type: "thread.message_queued",
						working_directory: "C:/work",
					});
					yield* InsertCanonicalJournalEvent("run-1-answer", "run-1", 3, {
						message_id: "answer-run-1",
						text: "settled answer",
						type: "assistant.message_completed",
					});
					yield* database.client.run(
						"UPDATE orchestration_runs SET status = 'completed' WHERE run_id = 'run-2'",
					);
					yield* database.client.insert(OrchestrationRuns).values({
						agent_id: "agent-other",
						created_at: now,
						engine_id: "engine-a",
						run_id: "run-other",
						status: "completed",
						thread_id: "thread-1",
						updated_at: now,
						working_directory: "C:/work",
					});
					yield* InsertCanonicalJournalEvent("other-user", "run-other", 4, {
						message_id: "message-other",
						text: "other agent",
						type: "thread.message_queued",
						working_directory: "C:/work",
					});
					yield* database.client.insert(OrchestrationRuns).values({
						agent_id: "agent-1",
						created_at: now,
						engine_id: "engine-a",
						run_id: "run-3",
						status: "queued",
						thread_id: "thread-1",
						updated_at: now,
						working_directory: "C:/work",
					});
					yield* InsertJournalEvent("run-3-start", "run-3", 5);
				}),
			);
			const repository = await repository_from(runtime);
			const run_2_context = await runtime.runPromise(repository.ReadContext("run-2"));
			const run_1_answer = (
				await runtime.runPromise(database.client.select().from(JournalEvents))
			).find((event) => event.event_id === "run-1-answer")!;
			expect(run_2_context.source_cut_journal_sequence).toBe(run_1_answer.sequence);
			const run_1_user = (
				await runtime.runPromise(database.client.select().from(JournalEvents))
			).find((event) => event.event_id === "run-1-user")!;
			const history = await runtime.runPromise(
				repository.ReadCanonicalHistory("run-3", {
					through_journal_sequence: run_1_user.sequence,
					through_run_id: "run-1",
				}),
			);
			expect(history.total_entries).toBe(2);
			expect(history.entries).toEqual([
				expect.objectContaining({
					role: "assistant",
					run_id: "run-1",
					text: "settled answer",
				}),
				expect.objectContaining({
					role: "user",
					run_id: "run-2",
					text: "queued follow-up",
				}),
			]);
		}));

	it("returns an exact canonical count and earliest objective while bounding entries", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(Seed());
			const database = await database_from(runtime);
			await runtime.runPromise(
				Effect.gen(function* () {
					yield* database.client.run("DELETE FROM journal_events");
					for (let index = 1; index <= 505; index++)
						yield* InsertCanonicalJournalEvent(
							`canonical-${index}`,
							"run-1",
							index,
							index % 2 === 0
								? {
										message_id: `assistant-${index}`,
										text: `answer ${index}`,
										type: "assistant.message_completed",
									}
								: {
										message_id: `user-${index}`,
										text:
											index === 1 ? "earliest objective" : `request ${index}`,
										type: "thread.message_queued",
										working_directory: "C:/work",
									},
						);
					yield* InsertJournalEvent("run-2-start", "run-2", 506);
				}),
			);
			const repository = await repository_from(runtime);
			const history = await runtime.runPromise(repository.ReadCanonicalHistory("run-2"));
			expect(history.total_entries).toBe(505);
			expect(history.entries).toHaveLength(500);
			expect(history.entries[0]?.logical_sequence).toBe(6);
			expect(Option.getOrUndefined(history.first_user_objective)?.text).toBe(
				"earliest objective",
			);
		}));

	it("rejects canonical rows whose envelope and payload types disagree", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(Seed());
			const database = await database_from(runtime);
			await runtime.runPromise(
				Effect.gen(function* () {
					yield* database.client.run("DELETE FROM journal_events");
					yield* InsertCanonicalJournalEvent("mismatched", "run-1", 1, {
						message_id: "assistant-mismatched",
						text: "not a user message",
						type: "assistant.message_completed",
					});
					yield* database.client.run(
						"UPDATE journal_events SET event_type = 'thread.message_queued' WHERE event_id = 'mismatched'",
					);
					yield* InsertJournalEvent("run-2-start", "run-2", 2);
				}),
			);
			const repository = await repository_from(runtime);
			await expect_failure_code(
				runtime.runPromise(repository.ReadCanonicalHistory("run-2")),
				"canonical_history_invalid",
			);
		}));

	it("uses typed conflict and failure error classes", () => {
		expect(new ThreadContinuationFailure({ code: "x" }).code).toBe("x");
		expect(new ThreadContinuationConflict({ target_run_id: "run" }).target_run_id).toBe("run");
	});
});
