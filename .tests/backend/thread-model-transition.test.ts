import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeCrypto } from "@effect/platform-node-shared";
import { Effect, Layer, ManagedRuntime, Schema } from "effect";
import { describe, expect, it } from "vitest";

import { EventEnvelope } from "@artisan/protocol";

import {
	ApplyEngineObservation,
	ApplyJournalEvent,
} from "../../modules/backend/src/conversation/projection-api";
import { make_database_layer, Database } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	ConversationItems,
	ConversationPatches,
	ConversationSources,
	ConversationThreads,
	ConversationTurns,
	JournalEvents,
	OrchestrationCoordinators,
	OrchestrationMessages,
	OrchestrationOutbox,
	OrchestrationRuns,
	Threads,
} from "../../modules/backend/src/persistence/tables";
import {
	ThreadContinuationRepository,
	ThreadContinuationRepositoryLive,
} from "../../modules/backend/src/persistence/thread-continuation/repository";
import { ThreadContinuationLaunches } from "../../modules/backend/src/persistence/thread-continuation-schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const now = "2026-07-30T10:00:00.000Z";

let minted = 0;

const make_runtime = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-model-transition-"));
	const database = make_database_layer({
		database_path: join(directory, "artisan.db"),
		migrations_path,
	});
	const metadata = Layer.succeed(
		RuntimeMetadata,
		RuntimeMetadata.of({
			instance_id: "backend-test",
			/** Distinct per call: one handoff appends two events, and both need their own id. */
			MakeId: (prefix) => Effect.sync(() => `${prefix}-test-${(minted += 1)}`),
			Now: Effect.succeed(now),
		}),
	);
	const base = Layer.mergeAll(database, metadata, JournalNotifierLive, NodeCrypto.layer);
	return {
		directory,
		runtime: ManagedRuntime.make(
			Layer.mergeAll(base, ThreadContinuationRepositoryLive.pipe(Layer.provide(base))),
		),
	};
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

const services = (runtime: TestRuntime) =>
	runtime.runPromise(
		Effect.gen(function* () {
			return {
				database: yield* Database,
				repository: yield* ThreadContinuationRepository,
			};
		}),
	);

const Seed = (target: { readonly engine_id: string; readonly model_id: string | null }) =>
	Effect.gen(function* () {
		const database = yield* Database;
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
			engine_id: target.engine_id,
			role: "coordinator",
			thread_id: "thread-1",
			updated_at: now,
		});
		yield* database.client.insert(OrchestrationRuns).values([
			{
				agent_id: "agent-1",
				created_at: now,
				engine_id: "claude",
				model_id: "claude-sonnet",
				native_resume_json: JSON.stringify({ native_thread_id: "source-native" }),
				native_thread_id: "source-native",
				run_id: "run-1",
				status: "completed",
				thread_id: "thread-1",
				updated_at: now,
				working_directory: "C:/work",
			},
			{
				agent_id: "agent-1",
				created_at: now,
				engine_id: target.engine_id,
				model_id: target.model_id,
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
			text: "Continue",
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
		yield* database.client.insert(JournalEvents).values([
			{
				causation_id: "event-1",
				correlation_id: "event-1",
				event_id: "event-1",
				event_type: "test",
				occurred_at: now,
				origin: "test",
				payload_json: "{}",
				run_id: "run-1",
				schema_version: 1,
				stream_id: "thread-1",
				stream_sequence: 1,
				thread_id: "thread-1",
			},
			{
				causation_id: "event-2",
				correlation_id: "event-2",
				event_id: "event-2",
				event_type: "test",
				occurred_at: now,
				origin: "test",
				payload_json: "{}",
				run_id: "run-2",
				schema_version: 1,
				stream_id: "thread-1",
				stream_sequence: 2,
				thread_id: "thread-1",
			},
		]);
	});

const BindNative = (repository: ThreadContinuationRepository["Service"]) =>
	Effect.gen(function* () {
		yield* repository.PrepareLaunch("run-2", {
			_tag: "native",
			request_id: "command-2",
			source_run_id: "run-1",
			target_model_id: "target-model",
		});
		yield* repository.MarkOpening("run-2");
		yield* repository.BindTarget({
			command_id: "command-2",
			model_id: "target-model",
			native_thread_id: "target-native",
			resume_token: { native_thread_id: "target-native" },
			target_run_id: "run-2",
		});
	});

describe("thread model transition journal projection", () => {
	it("appends one summary-free transition and rebuilds its renderer item from the journal", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(Seed({ engine_id: "claude", model_id: "target-model" }));
			const { database, repository } = await services(runtime);
			await runtime.runPromise(BindNative(repository));
			/** A bound replay validates its persisted target without re-reading retired source state. */
			await runtime.runPromise(
				database.client.run("DELETE FROM orchestration_runs WHERE run_id = 'run-1'"),
			);
			await runtime.runPromise(
				repository.BindTarget({
					command_id: "command-2",
					model_id: "target-model",
					native_thread_id: "target-native",
					resume_token: { native_thread_id: "target-native" },
					target_run_id: "run-2",
				}),
			);
			/** The handoff opens and lands; the landing is what the item settles on. */
			const [event] = (
				await runtime.runPromise(database.client.select().from(JournalEvents))
			).filter(
				(candidate) =>
					candidate.event_type === "thread.model_transition" &&
					JSON.parse(candidate.payload_json).state === "completed",
			);
			expect(event).toMatchObject({
				causation_id: "command-2",
				correlation_id: "command-2",
				event_type: "thread.model_transition",
				run_id: "run-2",
				thread_id: "thread-1",
			});
			expect(JSON.parse(event!.payload_json)).toEqual({
				continuation: "native",
				source: { engine_id: "claude", model_id: "claude-sonnet" },
				state: "completed",
				target: { engine_id: "claude", model_id: "target-model" },
				type: "thread.model_transition",
			});
			const [item] = await runtime.runPromise(
				database.client.select().from(ConversationItems),
			);
			expect(JSON.parse(item!.entity_json)).toMatchObject({
				continuation: "native",
				source_engine_id: "claude",
				source_model_id: "claude-sonnet",
				state: "completed",
				target_engine_id: "claude",
				target_model_id: "target-model",
				type: "model_transition",
			});
			/** One handoff is two events, so a faithful rebuild replays both in order. */
			const replay_events = (
				await runtime.runPromise(database.client.select().from(JournalEvents))
			)
				.filter((candidate) => candidate.event_type === "thread.model_transition")
				.map((candidate) =>
					Schema.decodeUnknownSync(EventEnvelope)({
						causation_id: candidate.causation_id,
						correlation_id: candidate.correlation_id,
						journal_sequence: candidate.sequence,
						kind: "event",
						message_id: candidate.event_id,
						origin: "backend",
						payload: JSON.parse(candidate.payload_json),
						protocol_version: 1,
						run_id: candidate.run_id!,
						schema_version: 1,
						sequence: candidate.stream_sequence,
						sent_at: candidate.occurred_at,
						stream_id: candidate.stream_id,
						thread_id: candidate.thread_id,
					}),
				);

			await runtime.runPromise(
				database.client.transaction((transaction) =>
					Effect.gen(function* () {
						yield* transaction.delete(ConversationPatches);
						yield* transaction.delete(ConversationSources);
						yield* transaction.delete(ConversationItems);
						yield* transaction.delete(ConversationTurns);
						yield* transaction.delete(ConversationThreads);
						for (const replay_event of replay_events) {
							yield* ApplyJournalEvent(transaction, replay_event) as Effect.Effect<
								void,
								never,
								never
							>;
						}
					}),
				),
			);
			const [rebuilt] = await runtime.runPromise(
				database.client.select().from(ConversationItems),
			);
			expect(rebuilt!.entity_json).toBe(item!.entity_json);
		}));

	it("records a portable cross-engine transition", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(Seed({ engine_id: "codex", model_id: "gpt-5" }));
			const { database, repository } = await services(runtime);
			await runtime.runPromise(
				database.client.insert(ThreadContinuationLaunches).values({
					created_at: now,
					handoff_id: "handoff-2",
					request_id: "command-2",
					source_kind: "portable",
					source_run_id: "run-1",
					state: "opening",
					target_engine_id: "codex",
					target_model_id: "gpt-5",
					target_run_id: "run-2",
					thread_id: "thread-1",
					updated_at: now,
				}),
			);
			await runtime.runPromise(
				repository.BindTarget({
					command_id: "command-2",
					model_id: "gpt-5",
					native_thread_id: "target-native",
					resume_token: { native_thread_id: "target-native" },
					target_run_id: "run-2",
				}),
			);
			const [event] = (
				await runtime.runPromise(database.client.select().from(JournalEvents))
			).filter((candidate) => candidate.event_type === "thread.model_transition");
			expect(JSON.parse(event!.payload_json)).toMatchObject({
				continuation: "portable",
				source: { engine_id: "claude", model_id: "claude-sonnet" },
				target: { engine_id: "codex", model_id: "gpt-5" },
			});
		}));

	it("uses one opaque compaction item for a provider-native lifecycle identity", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(Seed({ engine_id: "codex", model_id: "gpt-5" }));
			const { database } = await services(runtime);
			const context = {
				occurred_at: now,
				run_id: "run-2",
				thread_id: "thread-1",
			};
			await runtime.runPromise(
				database.client.transaction((transaction) =>
					Effect.gen(function* () {
						yield* ApplyEngineObservation(
							transaction,
							{
								_tag: "compaction",
								artisan_run_id: "run-2",
								compaction_id: "provider-native-compaction-id",
								observation_id: "compaction-start",
								raw: { engine_id: "codex", frame: {}, transport: "test" },
								sequence: 1,
								state: "started",
							},
							context,
						) as Effect.Effect<void, never, never>;
						yield* ApplyEngineObservation(
							transaction,
							{
								_tag: "compaction",
								artisan_run_id: "run-2",
								compaction_id: "provider-native-compaction-id",
								observation_id: "compaction-completed",
								raw: { engine_id: "codex", frame: {}, transport: "test" },
								sequence: 2,
								state: "completed",
								summary: "Compacted",
							},
							context,
						) as Effect.Effect<void, never, never>;
					}),
				),
			);
			const compacted = (
				await runtime.runPromise(database.client.select().from(ConversationItems))
			)
				.map((row) => JSON.parse(row.entity_json))
				.filter((item) => item.type === "compaction");
			expect(compacted).toHaveLength(1);
			expect(compacted[0]).toMatchObject({ state: "completed" });
			expect(compacted[0]!.id).not.toContain("provider-native-compaction-id");
		}));

	it("does not append a transition for an exact native resume", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(Seed({ engine_id: "claude", model_id: "claude-sonnet" }));
			const { database, repository } = await services(runtime);
			await runtime.runPromise(
				Effect.gen(function* () {
					yield* repository.PrepareLaunch("run-2", {
						_tag: "native",
						request_id: "command-2",
						source_run_id: "run-1",
						target_model_id: "claude-sonnet",
					});
					yield* repository.MarkOpening("run-2");
					yield* repository.BindTarget({
						command_id: "command-2",
						model_id: "claude-sonnet",
						native_thread_id: "target-native",
						resume_token: { native_thread_id: "target-native" },
						target_run_id: "run-2",
					});
				}),
			);
			expect(
				(await runtime.runPromise(database.client.select().from(JournalEvents))).filter(
					(event) => event.event_type === "thread.model_transition",
				),
			).toEqual([]);
		}));

	/**
	 * A handoff takes time: the next engine has to accept the thread before it
	 * can answer. Both ends are announced so the thread can say it is changing
	 * hands rather than looking like the new model is already thinking.
	 */
	it("announces the handoff opening and completes the same item when it lands", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(Seed({ engine_id: "codex", model_id: "gpt-5" }));
			const { database, repository } = await services(runtime);
			await runtime.runPromise(
				Effect.gen(function* () {
					yield* database.client.insert(ThreadContinuationLaunches).values({
						created_at: now,
						handoff_id: "handoff-2",
						request_id: "command-2",
						source_kind: "portable",
						source_run_id: "run-1",
						state: "prepared",
						target_engine_id: "codex",
						target_model_id: "gpt-5",
						target_run_id: "run-2",
						thread_id: "thread-1",
						updated_at: now,
					});
					yield* repository.MarkOpening("run-2");
				}),
			);
			const opening = (
				await runtime.runPromise(database.client.select().from(JournalEvents))
			).filter((event) => event.event_type === "thread.model_transition");

			expect(opening).toHaveLength(1);
			expect(JSON.parse(opening[0]!.payload_json)).toMatchObject({ state: "started" });

			await runtime.runPromise(
				repository.BindTarget({
					command_id: "command-2",
					model_id: "gpt-5",
					native_thread_id: "target-native",
					resume_token: { native_thread_id: "target-native" },
					target_run_id: "run-2",
				}),
			);
			const both = (
				await runtime.runPromise(database.client.select().from(JournalEvents))
			).filter((event) => event.event_type === "thread.model_transition");

			expect(both.map((event) => JSON.parse(event.payload_json).state)).toEqual([
				"started",
				"completed",
			]);
			/** One handoff, one row: the landing completes what the opening announced. */
			const items = (
				await runtime.runPromise(database.client.select().from(ConversationItems))
			).filter((item) => item.item_id.startsWith("model-transition:"));

			expect(items).toHaveLength(1);
		}));

	/**
	 * A run started on its engine's default keeps no model id. Treating that
	 * silence as a different model announced a switch nobody made.
	 */
	it("does not append a transition when the source never recorded its model", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(Seed({ engine_id: "claude", model_id: "claude-sonnet" }));
			const { database, repository } = await services(runtime);
			await runtime.runPromise(
				Effect.gen(function* () {
					yield* database.client.run(
						"UPDATE orchestration_runs SET model_id = NULL WHERE run_id = 'run-1'",
					);
					yield* repository.PrepareLaunch("run-2", {
						_tag: "native",
						request_id: "command-2",
						source_run_id: "run-1",
						target_model_id: "claude-sonnet",
					});
					yield* repository.MarkOpening("run-2");
					yield* repository.BindTarget({
						command_id: "command-2",
						model_id: "claude-sonnet",
						native_thread_id: "target-native",
						resume_token: { native_thread_id: "target-native" },
						target_run_id: "run-2",
					});
				}),
			);
			expect(
				(await runtime.runPromise(database.client.select().from(JournalEvents))).filter(
					(event) => event.event_type === "thread.model_transition",
				),
			).toEqual([]);
		}));

	/** An engine swap is still a change, even when the model it came from is unknown. */
	it("appends a transition for an engine change the source model cannot describe", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(Seed({ engine_id: "codex", model_id: "gpt-5-codex" }));
			const { database, repository } = await services(runtime);
			await runtime.runPromise(
				Effect.gen(function* () {
					yield* database.client.run(
						"UPDATE orchestration_runs SET model_id = NULL WHERE run_id = 'run-1'",
					);
					yield* database.client.insert(ThreadContinuationLaunches).values({
						created_at: now,
						handoff_id: "handoff-2",
						request_id: "command-2",
						source_kind: "portable",
						source_run_id: "run-1",
						state: "opening",
						target_engine_id: "codex",
						target_model_id: "gpt-5-codex",
						target_run_id: "run-2",
						thread_id: "thread-1",
						updated_at: now,
					});
					yield* repository.BindTarget({
						command_id: "command-2",
						model_id: "gpt-5-codex",
						native_thread_id: "target-native",
						resume_token: { native_thread_id: "target-native" },
						target_run_id: "run-2",
					});
				}),
			);
			expect(
				(await runtime.runPromise(database.client.select().from(JournalEvents))).filter(
					(event) => event.event_type === "thread.model_transition",
				),
			).toHaveLength(1);
		}));

	it("does not append a transition for a fresh launch", () =>
		with_runtime(async (runtime) => {
			await runtime.runPromise(Seed({ engine_id: "claude", model_id: "claude-sonnet" }));
			const { database, repository } = await services(runtime);
			await runtime.runPromise(
				Effect.gen(function* () {
					yield* database.client.run("DELETE FROM journal_events WHERE run_id = 'run-1'");
					yield* database.client.run(
						"DELETE FROM orchestration_runs WHERE run_id = 'run-1'",
					);
					yield* repository.PrepareLaunch("run-2", {
						_tag: "fresh",
						request_id: "command-2",
					});
					yield* repository.MarkOpening("run-2");
					yield* repository.BindTarget({
						command_id: "command-2",
						model_id: "claude-sonnet",
						native_thread_id: "target-native",
						resume_token: { native_thread_id: "target-native" },
						target_run_id: "run-2",
					});
				}),
			);
			expect(
				(await runtime.runPromise(database.client.select().from(JournalEvents))).filter(
					(event) => event.event_type === "thread.model_transition",
				),
			).toEqual([]);
		}));
});
