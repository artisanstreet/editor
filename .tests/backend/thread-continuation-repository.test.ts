import { createHash } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeCrypto } from "@effect/platform-node-shared";
import { Effect, Layer, ManagedRuntime, Option } from "effect";
import { describe, expect, it } from "vitest";

import {
	encode_portable_checkpoint_content,
	type PortableCheckpoint,
} from "../../modules/backend/src/orchestration/thread-continuation-model";
import { make_database_layer, Database } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	JournalEvents,
	OrchestrationCoordinators,
	OrchestrationMessages,
	OrchestrationOutbox,
	OrchestrationRuns,
	ThreadErasureClaims,
	Threads,
} from "../../modules/backend/src/persistence/tables";
import {
	ThreadContinuationConflict,
	ThreadContinuationRepository,
	ThreadContinuationRepositoryLive,
	ThreadContinuationFailure,
} from "../../modules/backend/src/persistence/thread-continuation/repository";
import {
	ThreadContinuationLaunches,
	ThreadPortableHandoffs,
	ThreadRunContinuationState,
} from "../../modules/backend/src/persistence/thread-continuation-schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

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
	const base = Layer.mergeAll(database, metadata, JournalNotifierLive, NodeCrypto.layer);
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
			const checkpoint = make_checkpoint({ method: "compaction_model_summary" });
			const launch = {
				_tag: "portable" as const,
				checkpoint,
				handoff_id: "handoff-1",
				lineage: {
					compactor_engine_id: "engine-a",
					compactor_model_id: "source-model",
					kind: "compactor" as const,
				},
				request_id: "command-2",
				source_run_id: "run-1",
				target_model_id: "target-model",
			};
			await expect_failure_code(
				runtime.runPromise(
					repository.PrepareLaunch("run-2", {
						...launch,
						lineage: { kind: "canonical" as const },
					}),
				),
				"portable_checkpoint_cut_mismatch",
			);
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
				method: "compaction_model_summary",
				omitted_entries: checkpoint.omitted_entries,
				provider_lineage_json: JSON.stringify(launch.lineage),
				summary: checkpoint.summary,
				tail_json: JSON.stringify(checkpoint.tail),
				through_journal_sequence: 1,
				through_observation_sequence: 7,
			});

			const changed_checkpoint = make_checkpoint({
				method: "compaction_model_summary",
				summary: "Different valid summary",
			});
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

	it("reads later logical runs in journal order despite interleaving", () =>
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
			const history = await runtime.runPromise(repository.ReadCanonicalHistory("run-3"));
			expect(history.total_entries).toBe(3);
			expect(history.entries).toEqual([
				expect.objectContaining({
					role: "user",
					run_id: "run-1",
				}),
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
