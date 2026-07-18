import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it } from "vitest";
import { NodeFileSystem } from "@effect/platform-node-shared";
import { Cause, Deferred, Effect, Exit, FileSystem, Layer, ManagedRuntime } from "effect";

import type { CommandEnvelope } from "@artisan/protocol";
import {
	make_backend_runtime,
	ProtocolRouter,
	ThreadRetentionClock,
	WorkspaceEvidenceRecorder,
	WorkspaceEvidenceRecorderLive,
} from "@artisan/backend";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	JournalStore,
	JournalStoreLive,
} from "../../modules/backend/src/persistence/journal-store";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const evidence_idempotency_migration = "20260713083812_workspace-evidence-idempotency";
const evidence_recorded_at = "2026-07-11T19:00:00.000Z";
const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-workspace-evidence-recorder-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

const MakePriorMigrationsPath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-workspace-evidence-prior-migrations-",
	});
	const prior_migrations_path = join(directory, "drizzle");
	const entries = yield* file_system.readDirectory(migrations_path);
	const prior_entries = entries.filter((entry) => entry < evidence_idempotency_migration);

	yield* Effect.sync(() => temporary_directories.push(directory));
	yield* file_system.makeDirectory(prior_migrations_path, { recursive: true });
	yield* Effect.forEach(
		prior_entries,
		(entry) =>
			file_system.copy(join(migrations_path, entry), join(prior_migrations_path, entry)),
		{ concurrency: "unbounded" },
	);

	return prior_migrations_path;
}).pipe(Effect.provide(NodeFileSystem.layer));

function make_metadata_layer(instance_id = "workspace_evidence_recorder_test") {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) => Effect.sync(() => `${instance_id}_${prefix}_${++next_id}`),
		Now: Effect.succeed(evidence_recorded_at),
	});
}

const retention_clock = Layer.succeed(ThreadRetentionClock, {
	Now: Effect.succeed(evidence_recorded_at),
});

interface EvidenceReadGate {
	readonly continue_recording: Deferred.Deferred<void>;
	readonly read_completed: Deferred.Deferred<void>;
}

function make_recorder_runtime(database_path: string, instance_id: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_metadata_layer(instance_id),
		JournalNotifierLive,
	);
	const journal = JournalStoreLive.pipe(Layer.provideMerge(infrastructure));
	const recorder = WorkspaceEvidenceRecorderLive.pipe(Layer.provide(journal));

	return ManagedRuntime.make(Layer.mergeAll(journal, recorder));
}

function make_gated_recorder_runtime(
	database_path: string,
	instance_id: string,
	gate: EvidenceReadGate,
) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_metadata_layer(instance_id),
		JournalNotifierLive,
	);
	const journal = JournalStoreLive.pipe(Layer.provideMerge(infrastructure));
	const gated_journal = Layer.effect(
		JournalStore,
		Effect.gen(function* () {
			const live = yield* JournalStore;
			let paused = false;

			return {
				...live,
				ReadCorrelatedEvents: (correlation_id: string) => {
					if (
						paused ||
						correlation_id !== "workspace_evidence:filesystem_evidence:correlation"
					) {
						return live.ReadCorrelatedEvents(correlation_id);
					}

					paused = true;

					return Effect.gen(function* () {
						const events = yield* live.ReadCorrelatedEvents(correlation_id);

						yield* Deferred.succeed(gate.read_completed, undefined);
						yield* Deferred.await(gate.continue_recording);

						return events;
					});
				},
			};
		}),
	).pipe(Layer.provide(journal));
	const recorder = WorkspaceEvidenceRecorderLive.pipe(Layer.provide(gated_journal));

	return ManagedRuntime.make(Layer.mergeAll(journal, recorder));
}

function make_create_command(thread_id: string): CommandEnvelope {
	return {
		kind: "command",
		message_id: `create_${thread_id}`,
		origin: "frontend",
		payload: {
			title: "Workspace evidence",
			type: "thread.create",
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-11T19:00:00.000Z",
		thread_id,
	};
}

async function create_thread(runtime: ReturnType<typeof make_backend_runtime>, thread_id: string) {
	await runtime.runPromise(
		Effect.gen(function* () {
			const router = yield* ProtocolRouter;

			yield* router.Route(make_create_command(thread_id));
		}),
	);
}

function filesystem_input(overrides: Partial<{ readonly path: string }> = {}) {
	return {
		agent_id: "agent_evidence",
		operation: "write" as const,
		operation_id: "filesystem_evidence",
		path: "C:/work/alpha/src/main.ts",
		raw_origin: { provider: "codex", reference: "call_42" },
		run_id: "run_evidence",
		thread_id: "thread_evidence",
		...overrides,
	};
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("WorkspaceEvidenceRecorder", () => {
	it("publishes content-free attributed evidence with deterministic trace identities", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			retention_clock,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			await create_thread(runtime, "thread_evidence");
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const recorder = yield* WorkspaceEvidenceRecorder;

					const filesystem = yield* recorder.RecordFilesystemMutation(filesystem_input());
					const process = yield* recorder.RecordProcessOwnership({
						operation_id: "process_evidence",
						source: "artisan_tool",
						thread_id: "thread_evidence",
						working_directory: "C:/work/alpha",
					});
					const git = yield* recorder.RecordGitWorkspaceObserved({
						branch: "feature/evidence",
						changed_file_count: 2,
						has_diff: true,
						operation_id: "git_evidence",
						root_path: "C:/work/alpha",
						thread_id: "thread_evidence",
						worktree_path: "C:/work/alpha",
					});

					return { filesystem, git, process };
				}),
			);

			expect(result.filesystem).toMatchObject({
				status: "accepted",
				event: {
					agent_id: "agent_evidence",
					causation_id: "workspace_evidence:filesystem_evidence:causation",
					correlation_id: "workspace_evidence:filesystem_evidence:correlation",
					payload: { type: "filesystem.mutation" },
					raw_origin: { provider: "codex", reference: "call_42" },
					run_id: "run_evidence",
				},
			});
			expect(result.process.event.payload.type).toBe("process.ownership");
			expect(result.git.event.payload.type).toBe("git.workspace.observed");
		} finally {
			await runtime.dispose();
		}
	});

	it("returns the original event for exact retries after a runtime restart", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			retention_clock,
			runtime_metadata: make_metadata_layer(),
		});

		await create_thread(first_runtime, "thread_evidence");
		const accepted = await first_runtime.runPromise(
			Effect.gen(function* () {
				const recorder = yield* WorkspaceEvidenceRecorder;

				return yield* recorder.RecordFilesystemMutation(filesystem_input());
			}),
		);

		await first_runtime.dispose();

		const second_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			retention_clock,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			const duplicate = await second_runtime.runPromise(
				Effect.gen(function* () {
					const recorder = yield* WorkspaceEvidenceRecorder;

					return yield* recorder.RecordFilesystemMutation(filesystem_input());
				}),
			);

			expect(duplicate).toEqual({ event: accepted.event, status: "duplicate" });
			await expect(
				second_runtime.runPromise(
					Effect.flatMap(WorkspaceEvidenceRecorder, (recorder) =>
						recorder.RecordFilesystemMutation(
							filesystem_input({ path: "C:/work/alpha/src/changed.ts" }),
						),
					),
				),
			).rejects.toMatchObject({
				_tag: "WorkspaceEvidenceConflict",
				operation_id: "filesystem_evidence",
			});
		} finally {
			await second_runtime.dispose();
		}
	});

	it("upgrades legacy duplicate evidence without deleting journal history", async () => {
		const database_path = await make_database_path();
		const prior_migrations_path = await Effect.runPromise(MakePriorMigrationsPath);
		const prior_runtime = ManagedRuntime.make(
			make_database_layer({ database_path, migrations_path: prior_migrations_path }),
		);

		await prior_runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.run(`
					INSERT INTO threads (thread_id, title, created_at, updated_at)
					VALUES ('thread_evidence', 'Workspace evidence', '2026-07-11T19:00:00.000Z', '2026-07-11T19:00:00.000Z')
				`);
				yield* database.client.run(`
					INSERT INTO event_streams (stream_id, last_sequence)
					VALUES ('thread:thread_evidence', 4)
				`);
				yield* database.client.run(`
					INSERT INTO journal_events (
						stream_id,
						stream_sequence,
						schema_version,
						event_id,
						correlation_id,
						causation_id,
						origin,
						raw_origin_json,
						event_type,
						thread_id,
						run_id,
						agent_id,
						payload_json,
						occurred_at
					)
					VALUES
						(
							'thread:thread_evidence', 1, 1, 'legacy_exact_first',
							'workspace_evidence:legacy_exact:correlation',
							'workspace_evidence:legacy_exact:causation',
							'backend', '{"provider":"codex","reference":"call_42"}',
							'filesystem.mutation', 'thread_evidence', 'run_evidence', 'agent_evidence',
							'{"operation":"write","path":"C:/work/alpha/src/main.ts","type":"filesystem.mutation"}',
							'2026-07-11T19:00:00.000Z'
						),
						(
							'thread:thread_evidence', 2, 1, 'legacy_exact_second',
							'workspace_evidence:legacy_exact:correlation',
							'workspace_evidence:legacy_exact:causation',
							'backend', '{"provider":"codex","reference":"call_42"}',
							'filesystem.mutation', 'thread_evidence', 'run_evidence', 'agent_evidence',
							'{"operation":"write","path":"C:/work/alpha/src/main.ts","type":"filesystem.mutation"}',
							'2026-07-11T19:00:00.000Z'
						),
						(
							'thread:thread_evidence', 3, 1, 'legacy_conflict_first',
							'workspace_evidence:legacy_conflict:correlation',
							'workspace_evidence:legacy_conflict:causation',
							'backend', '{"provider":"codex","reference":"call_42"}',
							'filesystem.mutation', 'thread_evidence', 'run_evidence', 'agent_evidence',
							'{"operation":"write","path":"C:/work/alpha/src/main.ts","type":"filesystem.mutation"}',
							'2026-07-11T19:00:00.000Z'
						),
						(
							'thread:thread_evidence', 4, 1, 'legacy_conflict_second',
							'workspace_evidence:legacy_conflict:correlation',
							'workspace_evidence:legacy_conflict:causation',
							'backend', '{"provider":"codex","reference":"call_42"}',
							'filesystem.mutation', 'thread_evidence', 'run_evidence', 'agent_evidence',
							'{"operation":"write","path":"C:/work/alpha/src/changed.ts","type":"filesystem.mutation"}',
							'2026-07-11T19:00:00.000Z'
						)
				`);
			}),
		);

		await prior_runtime.dispose();

		const upgraded_runtime = make_recorder_runtime(
			database_path,
			"workspace_evidence_upgraded",
		);

		try {
			const result = await upgraded_runtime.runPromise(
				Effect.gen(function* () {
					const journal = yield* JournalStore;
					const recorder = yield* WorkspaceEvidenceRecorder;
					const exact = yield* recorder.RecordFilesystemMutation({
						...filesystem_input(),
						operation_id: "legacy_exact",
					});
					const conflict = yield* Effect.exit(
						recorder.RecordFilesystemMutation({
							...filesystem_input(),
							operation_id: "legacy_conflict",
						}),
					);
					const exact_events = yield* journal.ReadCorrelatedEvents(
						"workspace_evidence:legacy_exact:correlation",
					);

					return { conflict, exact, exact_events };
				}),
			);

			expect(result.exact).toEqual({ event: result.exact_events[0], status: "duplicate" });
			expect(result.exact_events).toHaveLength(2);
			expect(Exit.isFailure(result.conflict)).toBe(true);

			if (Exit.isFailure(result.conflict)) {
				expect(Cause.squash(result.conflict.cause)).toMatchObject({
					_tag: "WorkspaceEvidenceConflict",
					operation_id: "legacy_conflict",
				});
			}
		} finally {
			await upgraded_runtime.dispose();
		}
	});

	it("rejects operation-id reuse when its intent or attribution changes", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			retention_clock,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			await create_thread(runtime, "thread_evidence");
			const recorder = await runtime.runPromise(WorkspaceEvidenceRecorder);

			await runtime.runPromise(recorder.RecordFilesystemMutation(filesystem_input()));

			const conflicts = [
				recorder.RecordFilesystemMutation(
					filesystem_input({ path: "C:/work/alpha/src/changed.ts" }),
				),
				recorder.RecordFilesystemMutation({
					...filesystem_input(),
					thread_id: "thread_changed",
				}),
				recorder.RecordFilesystemMutation({
					...filesystem_input(),
					agent_id: "agent_changed",
				}),
				recorder.RecordFilesystemMutation({
					...filesystem_input(),
					raw_origin: { provider: "claude", reference: "call_42" },
				}),
				recorder.RecordFilesystemMutation({
					...filesystem_input(),
					run_id: "run_changed",
				}),
				recorder.RecordProcessOwnership({
					operation_id: "filesystem_evidence",
					source: "artisan_tool",
					thread_id: "thread_evidence",
					working_directory: "C:/work/alpha",
				}),
			];

			for (const conflict of conflicts) {
				await expect(runtime.runPromise(conflict)).rejects.toMatchObject({
					_tag: "WorkspaceEvidenceConflict",
					operation_id: "filesystem_evidence",
				});
			}
		} finally {
			await runtime.dispose();
		}
	});

	it("serializes concurrent attempts before appending the same operation", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			retention_clock,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			await create_thread(runtime, "thread_evidence");
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const recorder = yield* WorkspaceEvidenceRecorder;
					const journal = yield* JournalStore;
					const attempts = yield* Effect.all(
						Array.from({ length: 4 }, () =>
							recorder.RecordFilesystemMutation(filesystem_input()),
						),
						{ concurrency: "unbounded" },
					);
					const events = yield* journal.ReadCorrelatedEvents(
						"workspace_evidence:filesystem_evidence:correlation",
					);

					return { attempts, events };
				}),
			);

			expect(result.attempts.map((attempt) => attempt.status)).toEqual([
				"accepted",
				"duplicate",
				"duplicate",
				"duplicate",
			]);
			expect(result.events).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("deduplicates synchronized evidence publication across runtime instances", async () => {
		const database_path = await make_database_path();
		const setup_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			retention_clock,
			runtime_metadata: make_metadata_layer("workspace_evidence_setup"),
		});

		await create_thread(setup_runtime, "thread_evidence");
		await setup_runtime.dispose();

		const continue_recording = await Effect.runPromise(Deferred.make<void>());
		const first_read_completed = await Effect.runPromise(Deferred.make<void>());
		const second_read_completed = await Effect.runPromise(Deferred.make<void>());
		const first_runtime = make_gated_recorder_runtime(
			database_path,
			"workspace_evidence_first",
			{
				continue_recording,
				read_completed: first_read_completed,
			},
		);
		const second_runtime = make_gated_recorder_runtime(
			database_path,
			"workspace_evidence_second",
			{
				continue_recording,
				read_completed: second_read_completed,
			},
		);

		try {
			await first_runtime.runPromise(WorkspaceEvidenceRecorder);
			await second_runtime.runPromise(WorkspaceEvidenceRecorder);

			const first_pending = first_runtime.runPromise(
				Effect.flatMap(WorkspaceEvidenceRecorder, (recorder) =>
					recorder.RecordFilesystemMutation(filesystem_input()),
				),
			);
			const second_pending = second_runtime.runPromise(
				Effect.flatMap(WorkspaceEvidenceRecorder, (recorder) =>
					recorder.RecordFilesystemMutation(filesystem_input()),
				),
			);

			await Effect.runPromise(
				Effect.all(
					[Deferred.await(first_read_completed), Deferred.await(second_read_completed)],
					{ concurrency: "unbounded" },
				),
			);
			await Effect.runPromise(Deferred.succeed(continue_recording, undefined));

			const results = await Promise.all([first_pending, second_pending]);
			const events = await first_runtime.runPromise(
				Effect.flatMap(JournalStore, (journal) =>
					journal.ReadCorrelatedEvents(
						"workspace_evidence:filesystem_evidence:correlation",
					),
				),
			);

			expect(results.map((result) => result.status).toSorted()).toEqual([
				"accepted",
				"duplicate",
			]);
			expect(results[0]!.event).toEqual(results[1]!.event);
			expect(events).toEqual([results[0]!.event]);
		} finally {
			await Effect.runPromise(Deferred.succeed(continue_recording, undefined));
			await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
		}
	});

	it("rejects malformed tool evidence before it reaches the journal", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
			retention_clock,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			await create_thread(runtime, "thread_evidence");
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const recorder = yield* WorkspaceEvidenceRecorder;
					const journal = yield* JournalStore;
					const invalid = yield* Effect.exit(
						recorder.RecordFilesystemMutation({
							...filesystem_input(),
							operation_id: " ",
						}),
					);
					const events = yield* journal.ReadReplay({ after_journal_sequence: 0 });

					return { events, invalid };
				}),
			);

			const error = Exit.isFailure(result.invalid)
				? Cause.squash(result.invalid.cause)
				: undefined;

			expect(error).toMatchObject({ _tag: "WorkspaceEvidenceInvalid" });
			expect(result.events.map(({ payload }) => payload.type)).not.toContain(
				"filesystem.mutation",
			);
		} finally {
			await runtime.dispose();
		}
	});
});
