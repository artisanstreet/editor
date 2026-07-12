import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";

import { Effect, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	JournalStore,
	JournalStoreLive,
} from "../../modules/backend/src/persistence/journal-store";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
	WorkspaceChangeOperations,
	WorkspaceChanges,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import {
	WorkspaceChangeRepository,
	WorkspaceChangeRepositoryLive,
} from "../../modules/backend/src/workspace/workspace-change-repository";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const before_identity = {
	algorithm: "sha256" as const,
	byte_count: 10,
	content_hash: "a".repeat(64),
};
const after_identity = {
	algorithm: "sha256" as const,
	byte_count: 12,
	content_hash: "b".repeat(64),
};

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-workspace-change-repository-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_metadata_layer(now: () => string = () => "2026-07-11T19:00:00.000Z") {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "workspace_change_repository_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.sync(now),
	});
}

function make_runtime(database_path: string, now?: () => string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_metadata_layer(now),
		JournalNotifierLive,
	);

	return ManagedRuntime.make(
		Layer.mergeAll(WorkspaceChangeRepositoryLive, JournalStoreLive).pipe(
			Layer.provideMerge(infrastructure),
		),
	);
}

function SeedThreads(
	database: Database["Service"],
	thread_ids: ReadonlyArray<string> = ["thread_1"],
) {
	return Effect.forEach(
		thread_ids,
		(thread_id) =>
			database.client.insert(Threads).values({
				created_at: "2026-07-11T19:00:00.000Z",
				thread_id,
				title: thread_id,
				title_source: "initial",
				updated_at: "2026-07-11T19:00:00.000Z",
			}),
		{ discard: true },
	);
}

function replace_claim(message_id = "message_replace", change_id = "change_1") {
	return {
		_tag: "replace" as const,
		agent_id: "agent_1",
		change_id,
		expected_before: before_identity,
		intended_after: after_identity,
		message_id,
		path: "src/example.ts",
		raw_origin: { provider: "codex", reference: "origin_1" },
		request_fingerprint: "c".repeat(64),
		run_id: "run_1",
		sent_at: "2026-07-11T19:00:00.000Z",
		thread_id: "thread_1",
		workspace_id: "workspace_1",
	};
}

function rollback_claim(message_id = "message_rollback", change_id = "change_1") {
	return {
		_tag: "rollback" as const,
		change_id,
		expected_after: after_identity,
		message_id,
		request_fingerprint: "f".repeat(64),
		sent_at: "2026-07-11T19:00:00.000Z",
		thread_id: "thread_1",
	};
}

function workspace_change_row(change_id: string, source_command_id: string) {
	return {
		after_identity_json: JSON.stringify(after_identity),
		agent_id: "agent_forged",
		before_identity_json: JSON.stringify(before_identity),
		change_id,
		created_at: "2026-07-11T19:00:00.000Z",
		path: "src/forged.ts",
		review_state: "needs_review",
		rollback_state: "available",
		run_id: "run_forged",
		source_command_id,
		thread_id: "thread_1",
		updated_at: "2026-07-11T19:00:00.000Z",
		version: 1,
		workspace_id: "workspace_forged",
	};
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("workspace change repository", () => {
	it("claims, applies, records, replays, decodes, and lists a content-free workspace change", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const journal = yield* JournalStore;
					const repository = yield* WorkspaceChangeRepository;
					const private_claim = {
						...replace_claim(),
						surrounding_context_after: "SURROUNDING_PRIVATE_AFTER",
						surrounding_context_before: "SURROUNDING_PRIVATE_BEFORE",
					};

					yield* SeedThreads(database);

					const claim = yield* repository.ClaimReplace(private_claim);
					const applied = yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_replace",
						result_identity: after_identity,
					});
					const committed = yield* repository.CommitRecorded("message_replace");
					const stored = yield* repository.ReadChange("change_1");
					const listed = yield* repository.List("thread_1", "workspace_1");
					const replay = yield* journal.ReadReplay({ after_journal_sequence: 0 });

					return {
						applied,
						claim,
						committed,
						listed,
						replay,
						persisted: {
							commands: yield* database.client.select().from(JournalCommands),
							events: yield* database.client.select().from(JournalEvents),
							operations: yield* database.client
								.select()
								.from(WorkspaceChangeOperations),
							changes: yield* database.client.select().from(WorkspaceChanges),
						},
						stored,
					};
				}),
			);

			expect(result.claim._tag).toBe("claimed");
			expect(result.applied.lifecycle).toBe("applied");
			expect(result.committed).toMatchObject({ status: "accepted" });
			expect(result.committed.event.payload).toMatchObject({
				action: "recorded",
				change: { version: 1 },
			});
			expect(result.stored._tag).toBe("Some");
			expect(result.listed).toMatchObject({
				changes: [expect.objectContaining({ change_id: "change_1" })],
				journal_sequence: 1,
			});
			expect(result.replay).toMatchObject([
				{
					origin: "backend",
					sequence: 1,
					stream_id: "thread:thread_1",
					thread_id: "thread_1",
					payload: { action: "recorded" },
				},
			]);
			expect(result.replay[0]).toMatchObject({
				agent_id: "agent_1",
				raw_origin: { provider: "codex", reference: "origin_1" },
				run_id: "run_1",
			});
			expect(result.persisted.commands).toHaveLength(1);
			expect(result.persisted.events).toHaveLength(1);
			expect(result.persisted.commands[0]).toMatchObject({
				agent_id: "agent_1",
				causation_id: null,
				origin: "frontend",
				raw_origin_json: '{"provider":"codex","reference":"origin_1"}',
				run_id: "run_1",
			});
			expect(JSON.stringify(result.persisted)).not.toContain("SURROUNDING_PRIVATE_BEFORE");
			expect(JSON.stringify(result.persisted)).not.toContain("SURROUNDING_PRIVATE_AFTER");
			expect(JSON.stringify(result.persisted)).not.toContain("C:/native/root");
			expect(JSON.stringify(result.persisted)).not.toContain("snapshot-path");
			expect("content" in result.claim.operation).toBe(false);
		} finally {
			await runtime.dispose();
		}
	});

	it("returns incomplete and committed exact retries while rejecting changed command or replace identities", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);

					yield* repository.ClaimReplace(replace_claim());
					const incomplete_retry = yield* repository.ClaimReplace(replace_claim());
					const conflict = yield* repository
						.ClaimReplace({ ...replace_claim(), request_fingerprint: "d".repeat(64) })
						.pipe(Effect.exit);
					const change_conflict = yield* repository
						.ClaimReplace(replace_claim("message_other", "change_1"))
						.pipe(Effect.exit);
					const timestamp_conflict = yield* repository
						.ClaimReplace({
							...replace_claim(),
							sent_at: "2026-07-11T19:00:01.000Z",
						})
						.pipe(Effect.exit);
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_replace",
						result_identity: after_identity,
					});
					yield* repository.CommitRecorded("message_replace");

					return {
						committed_retry: yield* repository.ClaimReplace(replace_claim()),
						committed_timestamp_conflict: yield* repository
							.ClaimReplace({
								...replace_claim(),
								sent_at: "2026-07-11T19:00:01.000Z",
							})
							.pipe(Effect.exit),
						change_conflict,
						conflict,
						incomplete_retry,
						timestamp_conflict,
					};
				}),
			);

			expect(result.incomplete_retry._tag).toBe("incomplete_retry");
			expect(result.committed_retry).toMatchObject({
				_tag: "duplicate",
				event: { payload: { action: "recorded" } },
			});
			expect(JSON.stringify(result.committed_timestamp_conflict)).toContain(
				"CommandIdConflict",
			);
			expect(JSON.stringify(result.conflict)).toContain("CommandIdConflict");
			expect(JSON.stringify(result.change_conflict)).toContain("WorkspaceChangeIdConflict");
			expect(JSON.stringify(result.timestamp_conflict)).toContain("CommandIdConflict");
		} finally {
			await runtime.dispose();
		}
	});

	it("applies review and rollback transitions with guarded state and thread checks", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database, ["thread_1", "thread_2"]);

					yield* repository.ClaimReplace(replace_claim());
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_replace",
						result_identity: after_identity,
					});
					yield* repository.CommitRecorded("message_replace");
					const wrong_thread = yield* repository
						.ClaimReview({
							_tag: "review",
							change_id: "change_1",
							message_id: "review_wrong_thread",
							request_fingerprint: "e".repeat(64),
							sent_at: "2026-07-11T19:00:00.000Z",
							thread_id: "thread_2",
						})
						.pipe(Effect.exit);
					yield* repository.ClaimReview({
						_tag: "review",
						change_id: "change_1",
						message_id: "review_1",
						request_fingerprint: "e".repeat(64),
						sent_at: "2026-07-11T19:00:00.000Z",
						thread_id: "thread_1",
					});
					const reviewed = yield* repository.CommitReviewed("review_1");
					const invalid = yield* repository
						.ClaimRollback({
							_tag: "rollback",
							change_id: "change_1",
							expected_after: before_identity,
							message_id: "rollback_invalid",
							request_fingerprint: "f".repeat(64),
							sent_at: "2026-07-11T19:00:00.000Z",
							thread_id: "thread_1",
						})
						.pipe(Effect.exit);
					return { invalid, reviewed, wrong_thread };
				}),
			);

			expect(result.reviewed.event.payload.change).toMatchObject({
				review_state: "reviewed",
				version: 2,
			});
			expect(JSON.stringify(result.invalid)).toContain("WorkspaceChangeTransitionError");
			expect(JSON.stringify(result.wrong_thread)).toContain("WorkspaceChangeTransitionError");
		} finally {
			await runtime.dispose();
		}
	});

	it("rolls back once, keeps evidence recording idempotent and rejects malformed stored JSON", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);

					yield* repository.ClaimReplace(replace_claim());
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_replace",
						result_identity: after_identity,
					});
					yield* repository.CommitRecorded("message_replace");
					const first_evidence =
						yield* repository.MarkEvidenceRecorded("message_replace");
					const second_evidence =
						yield* repository.MarkEvidenceRecorded("message_replace");
					const wrong_identity = yield* repository
						.ClaimRollback({
							_tag: "rollback",
							change_id: "change_1",
							expected_after: before_identity,
							message_id: "rollback_wrong_identity",
							request_fingerprint: "f".repeat(64),
							sent_at: "2026-07-11T19:00:00.000Z",
							thread_id: "thread_1",
						})
						.pipe(Effect.exit);
					yield* repository.ClaimRollback({
						_tag: "rollback",
						change_id: "change_1",
						expected_after: after_identity,
						message_id: "rollback_1",
						request_fingerprint: "f".repeat(64),
						sent_at: "2026-07-11T19:00:00.000Z",
						thread_id: "thread_1",
					});
					yield* repository.MarkApplied({ _tag: "rollback", message_id: "rollback_1" });
					const rolled_back = yield* repository.CommitRolledBack("rollback_1");
					const replace_only = yield* repository
						.MarkEvidenceRecorded("rollback_1")
						.pipe(Effect.exit);
					yield* database.client
						.update(WorkspaceChanges)
						.set({ after_identity_json: "{" });

					return {
						first_evidence,
						malformed: yield* repository.ReadChange("change_1").pipe(Effect.exit),
						replace_only,
						rolled_back,
						second_evidence,
						wrong_identity,
					};
				}),
			);

			expect(result.first_evidence.evidence_recorded).toBe(true);
			expect(result.second_evidence.evidence_recorded).toBe(true);
			expect(result.rolled_back.event.payload.change).toMatchObject({
				review_state: "rolled_back",
				rollback_state: "consumed",
				version: 2,
			});
			expect(JSON.stringify(result.malformed)).toContain("JournalInvariantError");
			expect(JSON.stringify(result.replace_only)).toContain("WorkspaceChangeTransitionError");
			expect(JSON.stringify(result.wrong_identity)).toContain(
				"WorkspaceChangeTransitionError",
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("serializes concurrent commits and preserves duplicates across a runtime restart", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_runtime(database_path);

		try {
			const result = await first_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);

					yield* repository.ClaimReplace(replace_claim());
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_replace",
						result_identity: after_identity,
					});
					const commits = yield* Effect.all(
						[
							repository.CommitRecorded("message_replace"),
							repository.CommitRecorded("message_replace"),
						],
						{ concurrency: "unbounded" },
					);

					return { commits, events: yield* database.client.select().from(JournalEvents) };
				}),
			);

			expect(result.commits.map((commit) => commit.status).sort()).toEqual([
				"accepted",
				"duplicate",
			]);
			expect(result.events).toHaveLength(1);
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_runtime(database_path);

		try {
			const retry = await second_runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* WorkspaceChangeRepository;

					return yield* repository.ClaimReplace(replace_claim());
				}),
			);

			expect(retry).toMatchObject({
				_tag: "duplicate",
				event: { payload: { action: "recorded" } },
			});
		} finally {
			await second_runtime.dispose();
		}
	});

	it("pins the intended replacement identity across restart and rejects changed observations", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_runtime(database_path);

		try {
			const claimed = await first_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);

					return yield* repository.ClaimReplace(replace_claim());
				}),
			);

			expect(claimed).toMatchObject({
				_tag: "claimed",
				operation: { action: "replace", result_identity: after_identity },
			});
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_runtime(database_path);

		try {
			const result = await second_runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* WorkspaceChangeRepository;
					const recovered = yield* repository.ReadOperation("message_replace");
					const different_identity = { ...after_identity, content_hash: "d".repeat(64) };
					const first_rejection = yield* repository
						.MarkApplied({
							_tag: "replace",
							message_id: "message_replace",
							result_identity: different_identity,
						})
						.pipe(Effect.exit);
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_replace",
						result_identity: after_identity,
					});
					const repeated_rejection = yield* repository
						.MarkApplied({
							_tag: "replace",
							message_id: "message_replace",
							result_identity: different_identity,
						})
						.pipe(Effect.exit);

					return { first_rejection, recovered, repeated_rejection };
				}),
			);

			expect(result.recovered).toMatchObject({
				_tag: "Some",
				value: { action: "replace", result_identity: after_identity },
			});
			expect(JSON.stringify(result.first_rejection)).toContain(
				"WorkspaceChangeTransitionError",
			);
			expect(JSON.stringify(result.repeated_rejection)).toContain(
				"WorkspaceChangeTransitionError",
			);
		} finally {
			await second_runtime.dispose();
		}
	});

	it("persists terminal changed rejections without creating journal state", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_runtime(database_path);

		try {
			const rejected = await first_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);
					yield* repository.ClaimReplace(replace_claim());

					return yield* repository.RejectChanged("message_replace");
				}),
			);

			expect(rejected).toMatchObject({
				evidence_recorded: false,
				lifecycle: "rejected",
			});
			expect(rejected).not.toHaveProperty("journal_sequence");
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_runtime(database_path);

		try {
			const result = await second_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					return {
						commands: yield* database.client.select().from(JournalCommands),
						changes: yield* database.client.select().from(WorkspaceChanges),
						events: yield* database.client.select().from(JournalEvents),
						operation: yield* repository.ReadOperation("message_replace"),
						retry: yield* repository.ClaimReplace(replace_claim()),
					};
				}),
			);

			expect(result.retry).toMatchObject({
				_tag: "rejected",
				operation: { lifecycle: "rejected" },
			});
			expect(result.operation).toMatchObject({
				_tag: "Some",
				value: { lifecycle: "rejected" },
			});
			expect(result.commands).toEqual([]);
			expect(result.events).toEqual([]);
			expect(result.changes).toEqual([]);
		} finally {
			await second_runtime.dispose();
		}
	});

	it("retries rejected rollbacks and keeps changed conflicts immutable", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);
					yield* repository.ClaimReplace(replace_claim());
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_replace",
						result_identity: after_identity,
					});
					yield* repository.CommitRecorded("message_replace");
					yield* repository.ClaimRollback(rollback_claim());
					yield* repository.RejectChanged("message_rollback");

					return {
						change_conflict: yield* repository
							.ClaimRollback(rollback_claim("message_rollback_other"))
							.pipe(Effect.exit),
						intent_conflict: yield* repository
							.ClaimRollback({
								...rollback_claim(),
								request_fingerprint: "e".repeat(64),
							})
							.pipe(Effect.exit),
						retry: yield* repository.ClaimRollback(rollback_claim()),
					};
				}),
			);

			expect(result.retry).toMatchObject({
				_tag: "rejected",
				operation: { action: "rollback", lifecycle: "rejected" },
			});
			expect(JSON.stringify(result.change_conflict)).toContain("WorkspaceChangeIdConflict");
			expect(JSON.stringify(result.intent_conflict)).toContain("CommandIdConflict");
		} finally {
			await runtime.dispose();
		}
	});

	it("makes changed rejection idempotent and rejects terminal or review transitions", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);
					yield* repository.ClaimReplace(replace_claim());
					const first_rejection = yield* repository.RejectChanged("message_replace");
					const duplicate_rejection = yield* repository.RejectChanged("message_replace");
					const applied = yield* repository
						.MarkApplied({
							_tag: "replace",
							message_id: "message_replace",
							result_identity: after_identity,
						})
						.pipe(Effect.exit);
					const committed = yield* repository
						.CommitRecorded("message_replace")
						.pipe(Effect.exit);

					yield* repository.ClaimReplace(replace_claim("message_committed", "change_2"));
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_committed",
						result_identity: after_identity,
					});
					yield* repository.CommitRecorded("message_committed");
					yield* repository.ClaimReplace(replace_claim("message_applied", "change_3"));
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_applied",
						result_identity: after_identity,
					});
					yield* repository.ClaimReview({
						_tag: "review",
						change_id: "change_2",
						message_id: "message_review",
						request_fingerprint: "d".repeat(64),
						sent_at: "2026-07-11T19:00:00.000Z",
						thread_id: "thread_1",
					});

					return {
						applied,
						committed,
						committed_rejection: yield* repository
							.RejectChanged("message_committed")
							.pipe(Effect.exit),
						duplicate_rejection,
						first_rejection,
						applied_rejection: yield* repository
							.RejectChanged("message_applied")
							.pipe(Effect.exit),
						review_rejection: yield* repository
							.RejectChanged("message_review")
							.pipe(Effect.exit),
					};
				}),
			);

			expect(result.first_rejection).toEqual(result.duplicate_rejection);
			for (const failure of [
				result.applied,
				result.applied_rejection,
				result.committed,
				result.committed_rejection,
				result.review_rejection,
			]) {
				expect(JSON.stringify(failure)).toContain("WorkspaceChangeTransitionError");
			}
		} finally {
			await runtime.dispose();
		}
	});

	it("fails closed when rejected operations conceal command or event state", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);
					yield* repository.ClaimReplace(replace_claim());
					yield* database.client.insert(JournalCommands).values({
						accepted_at: "2026-07-11T19:00:00.000Z",
						message_id: "message_replace",
						origin: "frontend",
						payload_json: '{"type":"thread.send_message"}',
						payload_type: "thread.send_message",
						schema_version: 1,
						sent_at: "2026-07-11T19:00:00.000Z",
						status: "accepted",
						thread_id: "thread_1",
					});
					const command_before_rejection = yield* repository
						.RejectChanged("message_replace")
						.pipe(Effect.exit);

					yield* database.client.delete(JournalCommands);
					yield* repository.RejectChanged("message_replace");
					yield* database.client.insert(JournalCommands).values({
						accepted_at: "2026-07-11T19:00:00.000Z",
						message_id: "message_replace",
						origin: "frontend",
						payload_json: '{"type":"thread.send_message"}',
						payload_type: "thread.send_message",
						schema_version: 1,
						sent_at: "2026-07-11T19:00:00.000Z",
						status: "accepted",
						thread_id: "thread_1",
					});
					const command_on_claim_retry = yield* repository
						.ClaimReplace(replace_claim())
						.pipe(Effect.exit);

					yield* database.client.delete(JournalCommands);
					yield* repository.ClaimReplace(replace_claim("message_event", "change_event"));
					yield* database.client.insert(JournalEvents).values({
						causation_id: "message_event",
						correlation_id: "message_event",
						event_id: "event_forged",
						event_type: "thread.created",
						occurred_at: "2026-07-11T19:00:00.000Z",
						origin: "backend",
						payload_json: '{"type":"thread.created"}',
						schema_version: 1,
						stream_id: "thread:thread_1",
						stream_sequence: 1,
						thread_id: "thread_1",
					});
					const event_before_rejection = yield* repository
						.RejectChanged("message_event")
						.pipe(Effect.exit);

					yield* database.client.delete(JournalEvents);
					yield* repository.RejectChanged("message_event");
					yield* database.client.insert(JournalEvents).values({
						causation_id: "message_event",
						correlation_id: "message_event",
						event_id: "event_forged_retry",
						event_type: "thread.created",
						occurred_at: "2026-07-11T19:00:00.000Z",
						origin: "backend",
						payload_json: '{"type":"thread.created"}',
						schema_version: 1,
						stream_id: "thread:thread_1",
						stream_sequence: 1,
						thread_id: "thread_1",
					});

					return {
						command_before_rejection,
						command_on_claim_retry,
						event_before_rejection,
						event_on_reject_retry: yield* repository
							.RejectChanged("message_event")
							.pipe(Effect.exit),
					};
				}),
			);

			for (const failure of Object.values(result)) {
				expect(JSON.stringify(failure)).toContain("JournalInvariantError");
			}
		} finally {
			await runtime.dispose();
		}
	});

	it("fails closed for replace projections under either immutable alias", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);
					yield* repository.ClaimReplace(replace_claim());
					yield* repository.RejectChanged("message_replace");
					yield* database.client
						.insert(WorkspaceChanges)
						.values(workspace_change_row("change_forged", "message_replace"));
					const source_command_retry = yield* repository
						.ClaimReplace(replace_claim())
						.pipe(Effect.exit);

					yield* database.client.delete(WorkspaceChanges);
					yield* repository.ClaimReplace(
						replace_claim("message_canonical", "change_canonical"),
					);
					yield* database.client
						.insert(WorkspaceChanges)
						.values(workspace_change_row("change_canonical", "message_forged"));
					const canonical_before_rejection = yield* repository
						.RejectChanged("message_canonical")
						.pipe(Effect.exit);

					yield* database.client.delete(WorkspaceChanges);
					yield* repository.RejectChanged("message_canonical");
					yield* database.client
						.insert(WorkspaceChanges)
						.values(workspace_change_row("change_canonical", "message_forged"));

					return {
						canonical_before_rejection,
						canonical_on_reject_retry: yield* repository
							.RejectChanged("message_canonical")
							.pipe(Effect.exit),
						source_command_retry,
					};
				}),
			);

			for (const failure of Object.values(result)) {
				expect(JSON.stringify(failure)).toContain("JournalInvariantError");
			}
		} finally {
			await runtime.dispose();
		}
	});

	it("fails closed when a rejected rollback projection has been consumed", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);
					yield* repository.ClaimReplace(replace_claim());
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_replace",
						result_identity: after_identity,
					});
					yield* repository.CommitRecorded("message_replace");
					yield* repository.ClaimRollback(rollback_claim());
					yield* repository.RejectChanged("message_rollback");
					yield* database.client.update(WorkspaceChanges).set({
						review_state: "rolled_back",
						rollback_state: "consumed",
						rolled_back_at: "2026-07-11T19:00:01.000Z",
						updated_at: "2026-07-11T19:00:01.000Z",
						version: 2,
					});

					return {
						claim_retry: yield* repository
							.ClaimRollback(rollback_claim())
							.pipe(Effect.exit),
						reject_retry: yield* repository
							.RejectChanged("message_rollback")
							.pipe(Effect.exit),
					};
				}),
			);

			for (const failure of Object.values(result)) {
				expect(JSON.stringify(failure)).toContain("WorkspaceChangeTransitionError");
			}
		} finally {
			await runtime.dispose();
		}
	});

	it("replays per-thread streams and permits review followed by rollback", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const journal = yield* JournalStore;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database, ["thread_1", "thread_2"]);

					yield* repository.ClaimReplace(replace_claim());
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_replace",
						result_identity: after_identity,
					});
					yield* repository.CommitRecorded("message_replace");
					yield* repository.ClaimReview({
						_tag: "review",
						change_id: "change_1",
						message_id: "review_1",
						request_fingerprint: "e".repeat(64),
						sent_at: "2026-07-11T19:00:00.000Z",
						thread_id: "thread_1",
					});
					yield* repository.CommitReviewed("review_1");
					yield* repository.ClaimRollback({
						_tag: "rollback",
						change_id: "change_1",
						expected_after: after_identity,
						message_id: "rollback_1",
						request_fingerprint: "f".repeat(64),
						sent_at: "2026-07-11T19:00:00.000Z",
						thread_id: "thread_1",
					});
					yield* repository.MarkApplied({ _tag: "rollback", message_id: "rollback_1" });
					const rolled_back = yield* repository.CommitRolledBack("rollback_1");
					const replace_retry = yield* repository.ClaimReplace(replace_claim());
					const review_retry = yield* repository.ClaimReview({
						_tag: "review",
						change_id: "change_1",
						message_id: "review_1",
						request_fingerprint: "e".repeat(64),
						sent_at: "2026-07-11T19:00:00.000Z",
						thread_id: "thread_1",
					});
					const rollback_retry = yield* repository.ClaimRollback({
						_tag: "rollback",
						change_id: "change_1",
						expected_after: after_identity,
						message_id: "rollback_1",
						request_fingerprint: "f".repeat(64),
						sent_at: "2026-07-11T19:00:00.000Z",
						thread_id: "thread_1",
					});
					yield* repository.ClaimReplace({
						...replace_claim("message_thread_2", "change_2"),
						thread_id: "thread_2",
					});
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_thread_2",
						result_identity: after_identity,
					});
					yield* repository.CommitRecorded("message_thread_2");

					return {
						replay: yield* journal.ReadReplay({ after_journal_sequence: 0 }),
						replace_retry,
						review_retry,
						rollback_retry,
						rolled_back,
					};
				}),
			);

			expect(result.rolled_back.event.payload.change).toMatchObject({
				review_state: "rolled_back",
				version: 3,
			});
			expect(result.replace_retry).toMatchObject({
				_tag: "duplicate",
				event: { payload: { action: "recorded" } },
			});
			expect(result.review_retry).toMatchObject({
				_tag: "duplicate",
				event: { payload: { action: "reviewed" } },
			});
			expect(result.rollback_retry).toMatchObject({
				_tag: "duplicate",
				event: { payload: { action: "rolled_back" } },
			});
			expect(result.replay).toMatchObject([
				{
					origin: "backend",
					sequence: 1,
					stream_id: "thread:thread_1",
					payload: { action: "recorded" },
				},
				{
					origin: "backend",
					sequence: 2,
					stream_id: "thread:thread_1",
					payload: { action: "reviewed" },
				},
				{
					origin: "backend",
					sequence: 3,
					stream_id: "thread:thread_1",
					payload: { action: "rolled_back" },
				},
				{
					origin: "backend",
					sequence: 1,
					stream_id: "thread:thread_2",
					payload: { action: "recorded" },
				},
			]);
			for (const replay_event of result.replay.slice(1, 3)) {
				expect(replay_event).not.toHaveProperty("agent_id");
				expect(replay_event).not.toHaveProperty("raw_origin");
				expect(replay_event).not.toHaveProperty("run_id");
			}
		} finally {
			await runtime.dispose();
		}
	});

	it("advances the established shared thread stream instead of creating a second stream", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);
					yield* database.client.insert(EventStreams).values({
						last_sequence: 7,
						stream_id: "thread:thread_1",
					});
					yield* repository.ClaimReplace(replace_claim());
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_replace",
						result_identity: after_identity,
					});
					const committed = yield* repository.CommitRecorded("message_replace");

					return {
						committed,
						streams: yield* database.client.select().from(EventStreams),
					};
				}),
			);

			expect(result.committed.event.sequence).toBe(8);
			expect(result.streams).toEqual([{ last_sequence: 8, stream_id: "thread:thread_1" }]);
		} finally {
			await runtime.dispose();
		}
	});

	it("uses one commit timestamp for the projection and event under a moving clock", async () => {
		const database_path = await make_database_path();
		const timestamps = [
			"2026-07-11T19:00:00.000Z",
			"2026-07-11T19:00:01.000Z",
			"2026-07-11T19:00:02.000Z",
			"2026-07-11T19:00:03.000Z",
		];
		let timestamp_index = 0;
		const runtime = make_runtime(
			database_path,
			() => timestamps[Math.min(timestamp_index++, timestamps.length - 1)]!,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);
					yield* repository.ClaimReplace(replace_claim());
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_replace",
						result_identity: after_identity,
					});
					const committed = yield* repository.CommitRecorded("message_replace");

					return {
						committed,
						retry: yield* repository.ClaimReplace(replace_claim()),
					};
				}),
			);

			expect(result.committed.event).toMatchObject({
				occurred_at: "2026-07-11T19:00:02.000Z",
				payload: {
					change: {
						created_at: "2026-07-11T19:00:02.000Z",
						updated_at: "2026-07-11T19:00:02.000Z",
					},
				},
			});
			expect(result.retry._tag).toBe("duplicate");
		} finally {
			await runtime.dispose();
		}
	});

	it("fences apply, commit, retry, and list operations once thread erasure begins", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);
					yield* repository.ClaimReplace(replace_claim());
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-11T19:00:01.000Z",
						thread_id: "thread_1",
					});

					const apply_fenced = yield* repository
						.MarkApplied({
							_tag: "replace",
							message_id: "message_replace",
							result_identity: after_identity,
						})
						.pipe(Effect.exit);

					yield* database.client.delete(ThreadErasureClaims);
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_replace",
						result_identity: after_identity,
					});
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-11T19:00:02.000Z",
						thread_id: "thread_1",
					});

					const commit_fenced = yield* repository
						.CommitRecorded("message_replace")
						.pipe(Effect.exit);
					const retry_fenced = yield* repository
						.ClaimReplace(replace_claim())
						.pipe(Effect.exit);
					const list_fenced = yield* repository.List("thread_1").pipe(Effect.exit);

					yield* database.client.delete(ThreadErasureClaims);
					yield* database.client.delete(Threads);
					yield* database.client.insert(ThreadTombstones).values({
						deleted_at: "2026-07-11T19:00:03.000Z",
						thread_id: "thread_1",
					});
					const tombstoned = yield* repository
						.ClaimReplace(replace_claim("message_tombstoned", "change_tombstoned"))
						.pipe(Effect.exit);

					return {
						apply_fenced,
						commit_fenced,
						journal_commands: yield* database.client.select().from(JournalCommands),
						journal_events: yield* database.client.select().from(JournalEvents),
						list_fenced,
						retry_fenced,
						tombstoned,
						workspace_changes: yield* database.client.select().from(WorkspaceChanges),
					};
				}),
			);

			for (const failure of [
				result.apply_fenced,
				result.commit_fenced,
				result.list_fenced,
				result.retry_fenced,
				result.tombstoned,
			]) {
				expect(JSON.stringify(failure)).toContain("WorkspaceChangeTransitionError");
			}
			expect(result.journal_commands).toEqual([]);
			expect(result.journal_events).toEqual([]);
			expect(result.workspace_changes).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("fences changed rejection once thread erasure begins or completes", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);
					yield* repository.ClaimReplace(replace_claim());
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-11T19:00:01.000Z",
						thread_id: "thread_1",
					});

					const claimed_fence = yield* repository
						.RejectChanged("message_replace")
						.pipe(Effect.exit);

					yield* database.client.delete(ThreadErasureClaims);
					yield* database.client.delete(Threads);
					yield* database.client.insert(ThreadTombstones).values({
						deleted_at: "2026-07-11T19:00:02.000Z",
						thread_id: "thread_1",
					});

					return {
						claimed_fence,
						tombstone_fence: yield* repository
							.RejectChanged("message_replace")
							.pipe(Effect.exit),
					};
				}),
			);

			for (const failure of [result.claimed_fence, result.tombstone_fence]) {
				expect(JSON.stringify(failure)).toContain("WorkspaceChangeTransitionError");
			}
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects corrupted operation and projection lifecycle combinations", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);
					yield* repository.ClaimReplace(replace_claim());
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_replace",
						result_identity: after_identity,
					});
					yield* repository.CommitRecorded("message_replace");
					yield* database.client
						.update(WorkspaceChangeOperations)
						.set({ lifecycle: "claimed" });
					const operation = yield* repository
						.ReadOperation("message_replace")
						.pipe(Effect.exit);

					yield* database.client
						.update(WorkspaceChangeOperations)
						.set({ lifecycle: "committed" });
					yield* database.client
						.update(WorkspaceChanges)
						.set({ review_state: "reviewed" });
					const projection = yield* repository.ReadChange("change_1").pipe(Effect.exit);

					return { operation, projection };
				}),
			);

			expect(JSON.stringify(result.operation)).toContain("JournalInvariantError");
			expect(JSON.stringify(result.projection)).toContain("JournalInvariantError");
		} finally {
			await runtime.dispose();
		}
	});

	it("fails closed for malformed rejected operation state", async () => {
		const runtime = make_runtime(await make_database_path());

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);
					yield* repository.ClaimReplace(replace_claim());
					yield* repository.RejectChanged("message_replace");
					yield* database.client
						.update(WorkspaceChangeOperations)
						.set({ journal_sequence: 1 });
					const journal_sequence = yield* repository
						.ReadOperation("message_replace")
						.pipe(Effect.exit);

					yield* database.client
						.update(WorkspaceChangeOperations)
						.set({ evidence_recorded: true, journal_sequence: null });
					const evidence_recorded = yield* repository
						.ReadOperation("message_replace")
						.pipe(Effect.exit);

					yield* database.client
						.update(WorkspaceChangeOperations)
						.set({ agent_id: null, evidence_recorded: false });
					const action_shape = yield* repository
						.ReadOperation("message_replace")
						.pipe(Effect.exit);

					yield* repository.ClaimReplace(replace_claim("message_committed", "change_2"));
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_committed",
						result_identity: after_identity,
					});
					yield* repository.CommitRecorded("message_committed");
					yield* repository.ClaimReview({
						_tag: "review",
						change_id: "change_2",
						message_id: "message_review",
						request_fingerprint: "d".repeat(64),
						sent_at: "2026-07-11T19:00:00.000Z",
						thread_id: "thread_1",
					});
					yield* database.client
						.update(WorkspaceChangeOperations)
						.set({ lifecycle: "rejected" });
					const review = yield* repository
						.ReadOperation("message_review")
						.pipe(Effect.exit);

					return { action_shape, evidence_recorded, journal_sequence, review };
				}),
			);

			for (const failure of Object.values(result)) {
				expect(JSON.stringify(failure)).toContain("JournalInvariantError");
			}
		} finally {
			await runtime.dispose();
		}
	});

	it("converges concurrent changed rejections for one message", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_runtime(database_path);
		const second_runtime = make_runtime(database_path);

		try {
			await first_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);
					yield* repository.ClaimReplace(replace_claim());
				}),
			);

			const rejections = await Promise.all([
				first_runtime.runPromise(
					Effect.service(WorkspaceChangeRepository).pipe(
						Effect.flatMap((repository) => repository.RejectChanged("message_replace")),
					),
				),
				second_runtime.runPromise(
					Effect.service(WorkspaceChangeRepository).pipe(
						Effect.flatMap((repository) => repository.RejectChanged("message_replace")),
					),
				),
			]);

			expect(rejections).toEqual([rejections[0], rejections[0]]);
			expect(rejections[0]).toMatchObject({ lifecycle: "rejected" });
		} finally {
			await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
		}
	}, 10_000);

	it("rejects a workspace claim that collides with an existing journal command", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const collision = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);
					yield* database.client.insert(JournalCommands).values({
						accepted_at: "2026-07-11T19:00:00.000Z",
						message_id: "message_replace",
						origin: "frontend",
						payload_json: '{"type":"thread.send_message"}',
						payload_type: "thread.send_message",
						schema_version: 1,
						sent_at: "2026-07-11T19:00:00.000Z",
						status: "accepted",
						thread_id: "thread_1",
					});

					return yield* repository.ClaimReplace(replace_claim()).pipe(Effect.exit);
				}),
			);

			expect(JSON.stringify(collision)).toContain("CommandIdConflict");
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects exact duplicates with forged command attribution or event ownership", async () => {
		const database_path = await make_database_path();
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;

					yield* SeedThreads(database);

					yield* repository.ClaimReplace(replace_claim());
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_replace",
						result_identity: after_identity,
					});
					const committed = yield* repository.CommitRecorded("message_replace");
					yield* database.client.update(JournalCommands).set({
						payload_json: '{"type":"workspace.change.command","action":"review"}',
					});
					const command_identity = yield* repository
						.ClaimReplace(replace_claim())
						.pipe(Effect.exit);

					yield* database.client.update(JournalCommands).set({
						agent_id: "forged_agent",
						payload_json: JSON.stringify({
							action: "replace",
							change_id: "change_1",
							request_fingerprint: "c".repeat(64),
							type: "workspace.change.command",
						}),
					});
					const command_attribution = yield* repository
						.ClaimReplace(replace_claim())
						.pipe(Effect.exit);

					yield* database.client.update(JournalCommands).set({ agent_id: "agent_1" });
					yield* database.client
						.update(JournalEvents)
						.set({ event_type: "forged.event" });
					const event_type = yield* repository
						.ClaimReplace(replace_claim())
						.pipe(Effect.exit);

					yield* database.client.update(JournalEvents).set({
						causation_id: "forged_cause",
						event_type: "workspace.change.updated",
					});
					const event_causation = yield* repository
						.ClaimReplace(replace_claim())
						.pipe(Effect.exit);

					yield* database.client.update(JournalEvents).set({
						causation_id: "message_replace",
						payload_json: JSON.stringify({
							...committed.event.payload,
							change: {
								...committed.event.payload.change,
								source_command_id: "forged_command",
							},
						}),
					});
					const event_payload = yield* repository
						.ClaimReplace(replace_claim())
						.pipe(Effect.exit);

					return {
						command_attribution,
						command_identity,
						event_causation,
						event_payload,
						event_type,
					};
				}),
			);

			for (const failure of Object.values(result)) {
				expect(JSON.stringify(failure)).toContain("JournalInvariantError");
			}
		} finally {
			await runtime.dispose();
		}
	});
});
