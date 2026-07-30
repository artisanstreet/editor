import { createHash } from "node:crypto";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Cause, Effect, Exit, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	JournalCommands,
	JournalEvents,
	Threads,
	WorkspaceChangeDiffs,
	WorkspaceChangeOperations,
	WorkspaceChanges,
	WorkspaceMutationAuthorities,
	WorkspaceMutationPayloads,
} from "../../modules/backend/src/persistence/tables";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";
import {
	WorkspaceChangeRepository,
	WorkspaceChangeRepositoryLive,
} from "../../modules/backend/src/workspace/changes/repository";
import {
	WorkspaceChangeDiffInvalid,
	WorkspaceChangeDiffLimit,
	WorkspaceChangeDiffService,
	WorkspaceChangeDiffServiceLive,
	WorkspaceChangeDiffUnavailable,
} from "../../modules/backend/src/workspace/changes/diff";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const workspace_diff_migration = "20260713095034_lively_betty_brant";
const temporary_directories: Array<string> = [];
const text_encoder = new TextEncoder();

const before = "alpha\nbeta\nomega\n";
const after = "alpha\nbravo\nomega\n";
const expected_patch =
	"--- a/src/example.ts\n+++ b/src/example.ts\n@@ -1,3 +1,3 @@\n alpha\n-beta\n+bravo\n omega\n";

function identity(content: Uint8Array) {
	return {
		algorithm: "sha256" as const,
		byte_count: content.byteLength,
		content_hash: createHash("sha256").update(content).digest("hex"),
	};
}

function failure_from(exit: Exit.Exit<unknown, unknown>) {
	if (Exit.isFailure(exit)) {
		return Cause.squash(exit.cause);
	}

	throw new Error("Expected the Effect to fail");
}

function make_metadata_layer(instance_id: string) {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) => Effect.sync(() => `${instance_id}_${prefix}_${++next_id}`),
		Now: Effect.succeed("2026-07-13T12:00:00.000Z"),
	});
}

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-workspace-change-diff-",
	});

	yield* Effect.sync(() => temporary_directories.push(directory));

	return join(directory, "artisan.db");
}).pipe(Effect.provide(NodeFileSystem.layer));

const MakePriorMigrationsPath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-workspace-change-diff-prior-migrations-",
	});
	const prior_migrations_path = join(directory, "drizzle");
	const entries = yield* file_system.readDirectory(migrations_path);
	const prior_entries = entries.filter((entry) => entry < workspace_diff_migration);

	yield* Effect.sync(() => temporary_directories.push(directory));
	yield* file_system.makeDirectory(prior_migrations_path, { recursive: true });
	yield* Effect.forEach(
		prior_entries,
		(entry) =>
			file_system.copy(join(migrations_path, entry), join(prior_migrations_path, entry)),
		{ concurrency: "unbounded", discard: true },
	);

	return prior_migrations_path;
}).pipe(Effect.provide(NodeFileSystem.layer));

function make_runtime(database_path: string, instance_id: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_metadata_layer(instance_id),
		JournalNotifierLive,
	);
	const services = Layer.mergeAll(
		WorkspaceChangeDiffServiceLive,
		WorkspaceChangeRepositoryLive,
	).pipe(Layer.provideMerge(NodeCrypto.layer), Layer.provideMerge(infrastructure));

	return ManagedRuntime.make(services);
}

function make_database_runtime(database_path: string, migration_path: string) {
	return ManagedRuntime.make(
		make_database_layer({ database_path, migrations_path: migration_path }),
	);
}

function replace_claim(message_id = "message_1", change_id = "change_1") {
	return {
		_tag: "replace" as const,
		agent_id: "agent_1",
		change_id,
		expected_before: identity(text_encoder.encode(before)),
		intended_after: identity(text_encoder.encode(after)),
		message_id,
		path: "src/example.ts",
		raw_origin: { provider: "codex", reference: "origin_1" },
		request_fingerprint: "a".repeat(64),
		run_id: "run_1",
		sent_at: "2026-07-13T12:00:00.000Z",
		thread_id: "thread_1",
		workspace_id: "workspace_1",
	};
}

function prepare_input(overrides: Partial<ReturnType<typeof replace_claim>> = {}) {
	const claim = { ...replace_claim(), ...overrides };

	return {
		after: text_encoder.encode(after),
		after_identity: claim.intended_after,
		before: text_encoder.encode(before),
		before_identity: claim.expected_before,
		change_id: claim.change_id,
		message_id: claim.message_id,
		path: claim.path,
		thread_id: claim.thread_id,
		workspace_id: claim.workspace_id,
	};
}

const SeedThread = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.insert(Threads).values({
		created_at: "2026-07-13T12:00:00.000Z",
		thread_id: "thread_1",
		title: "Diff test thread",
		title_source: "initial",
		updated_at: "2026-07-13T12:00:00.000Z",
	});
});

function Prepare(input = prepare_input()) {
	return Effect.gen(function* () {
		const service = yield* WorkspaceChangeDiffService;

		return yield* service.Prepare(input);
	});
}

function CommitRecorded(message_id = "message_1", change_id = "change_1") {
	return Effect.gen(function* () {
		const repository = yield* WorkspaceChangeRepository;
		const prepared = yield* Prepare(prepare_input(replace_claim(message_id, change_id)));

		yield* repository.ClaimReplace(replace_claim(message_id, change_id));
		yield* repository.MarkApplied({
			_tag: "replace",
			message_id,
			result_identity: identity(text_encoder.encode(after)),
		});

		return yield* repository.CommitRecorded(message_id, prepared);
	});
}

function ReadDiff(change_id = "change_1") {
	return Effect.gen(function* () {
		const service = yield* WorkspaceChangeDiffService;

		return yield* service.Read({ change_id, thread_id: "thread_1" });
	});
}

const ReadRows = Effect.gen(function* () {
	const database = yield* Database;

	return {
		changes: yield* database.client.select().from(WorkspaceChanges),
		commands: yield* database.client.select().from(JournalCommands),
		diffs: yield* database.client.select().from(WorkspaceChangeDiffs),
		events: yield* database.client.select().from(JournalEvents),
		operations: yield* database.client.select().from(WorkspaceChangeOperations),
	};
});

const InstallDiffInsertFailure = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.run(`
		CREATE TRIGGER fail_workspace_change_diff_insert
		BEFORE INSERT ON workspace_change_diffs
		WHEN NEW.change_id = 'change_1'
		BEGIN
			SELECT RAISE(ABORT, 'deterministic diff insert failure');
		END
	`);
});

const RemoveDiffInsertFailure = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.run("DROP TRIGGER IF EXISTS fail_workspace_change_diff_insert");
});

afterEach(async () => {
	await Effect.runPromise(
		Effect.forEach(
			temporary_directories.splice(0),
			(directory) =>
				Effect.service(FileSystem.FileSystem).pipe(
					Effect.flatMap((file_system) =>
						file_system.remove(directory, { force: true, recursive: true }),
					),
				),
			{ concurrency: "unbounded", discard: true },
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("WorkspaceChangeDiffService and WorkspaceChangeRepository", () => {
	it("prepares a deterministic V1 patch with exact headers, counts, hash, and replay bytes", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "prepare");

		try {
			const [first, second] = await runtime.runPromise(Effect.all([Prepare(), Prepare()]));

			expect(new TextDecoder().decode(first.patch)).toBe(expected_patch);
			expect(first).toMatchObject({
				added_line_count: 1,
				context_lines: 3,
				format: "unified",
				format_version: 1,
				patch_identity: {
					byte_count: expected_patch.length,
					content_hash:
						"b381321f94934dfaf54dab365a411dbe6da7ee54b72b61788acac5a87c9da583",
				},
				removed_line_count: 1,
			});
			expect(second).toEqual(first);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects mismatched identities, malformed UTF-8, and edit-budget exhaustion deterministically", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "limits");
		const repeated_before = Array.from(
			{ length: 50_000 },
			(_, index) => `before-${index}`,
		).join("\n");
		const repeated_after = Array.from({ length: 50_000 }, (_, index) => `after-${index}`).join(
			"\n",
		);

		try {
			const mismatched = await runtime.runPromise(
				Prepare({
					...prepare_input(),
					before_identity: {
						...identity(text_encoder.encode(before)),
						content_hash: "f".repeat(64),
					},
				}).pipe(Effect.exit),
			);
			const malformed_bytes = Uint8Array.from([0xc3, 0x28]);
			const malformed = await runtime.runPromise(
				Prepare({
					...prepare_input(),
					before: malformed_bytes,
					before_identity: identity(malformed_bytes),
				}).pipe(Effect.exit),
			);
			const limited = await runtime.runPromise(
				Prepare({
					after: text_encoder.encode(repeated_after),
					after_identity: identity(text_encoder.encode(repeated_after)),
					before: text_encoder.encode(repeated_before),
					before_identity: identity(text_encoder.encode(repeated_before)),
					change_id: "change_budget",
					message_id: "message_budget",
					path: "src/budget.ts",
					thread_id: "thread_1",
					workspace_id: "workspace_1",
				}).pipe(Effect.exit),
			);

			expect(failure_from(mismatched)).toEqual(
				new WorkspaceChangeDiffInvalid({ reason: "identity" }),
			);
			expect(failure_from(malformed)).toEqual(
				new WorkspaceChangeDiffInvalid({ reason: "utf8" }),
			);
			expect(failure_from(limited)).toEqual(
				new WorkspaceChangeDiffLimit({ limit: "edit_length" }),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps the worst accepted full rewrite inside the V1 synchronous latency guard", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "latency");
		const before_text = Array.from({ length: 1_000 }, (_, index) => `before-${index}`).join(
			"\n",
		);
		const after_text = Array.from({ length: 1_000 }, (_, index) => `after-${index}`).join("\n");
		const started_at = performance.now();

		try {
			const prepared = await runtime.runPromise(
				Prepare({
					after: text_encoder.encode(after_text),
					after_identity: identity(text_encoder.encode(after_text)),
					before: text_encoder.encode(before_text),
					before_identity: identity(text_encoder.encode(before_text)),
					change_id: "change_latency",
					message_id: "message_latency",
					path: "src/latency.ts",
					thread_id: "thread_1",
					workspace_id: "workspace_1",
				}),
			);
			const elapsed_milliseconds = performance.now() - started_at;

			expect(prepared).toMatchObject({
				added_line_count: 1_000,
				removed_line_count: 1_000,
			});
			expect(elapsed_milliseconds).toBeLessThan(750);
		} finally {
			await runtime.dispose();
		}
	}, 5_000);

	it("commits an available patch atomically and reads the same artifact after a runtime restart", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const first_runtime = make_runtime(database_path, "first");

		try {
			const result = await first_runtime.runPromise(
				Effect.andThen(SeedThread, CommitRecorded()),
			);

			expect(result.status).toBe("accepted");
		} finally {
			await first_runtime.dispose();
		}

		const restarted_runtime = make_runtime(database_path, "restarted");

		try {
			const read = await restarted_runtime.runPromise(ReadDiff());

			expect(read).toMatchObject({
				patch: expected_patch,
				truncated: false,
			});
		} finally {
			await restarted_runtime.dispose();
		}
	});

	it("roundtrips quoted Unicode workspace paths through commit, evidence, duplicate, and read", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "quoted-path");
		const path = 'src/quoted-\u00e9"name.ts';
		const claim = { ...replace_claim("message_quoted", "change_quoted"), path };

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					const repository = yield* WorkspaceChangeRepository;
					const prepared = yield* Prepare(prepare_input(claim));

					yield* repository.ClaimReplace(claim);
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: claim.message_id,
						result_identity: claim.intended_after,
					});
					const committed = yield* repository.CommitRecorded(claim.message_id, prepared);
					const evidence = yield* repository.MarkEvidenceRecorded(claim.message_id);
					const duplicate = yield* repository.CommitRecorded(claim.message_id, prepared);
					const read = yield* ReadDiff(claim.change_id);

					return { committed, duplicate, evidence, read };
				}),
			);

			expect(result.committed.status).toBe("accepted");
			expect(result.duplicate.status).toBe("duplicate");
			expect(result.evidence.evidence_recorded).toBe(true);
			expect(result.read).toMatchObject({ path, patch: expect.stringContaining("@@") });
		} finally {
			await runtime.dispose();
		}
	});

	it("fails available projections closed when their artifact is missing or corrupt, while preserving legacy unavailable", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "unavailable");

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					yield* CommitRecorded();
					const database = yield* Database;

					yield* database.client.run(
						"DELETE FROM workspace_change_diffs WHERE change_id = 'change_1'",
					);
					const missing = yield* ReadDiff().pipe(Effect.exit);
					yield* database.client.insert(WorkspaceChanges).values({
						after_identity_json: JSON.stringify(identity(text_encoder.encode(after))),
						agent_id: "agent_legacy",
						before_identity_json: JSON.stringify(identity(text_encoder.encode(before))),
						change_id: "change_legacy",
						created_at: "2026-07-13T12:00:00.000Z",
						path: "src/legacy.ts",
						review_state: "needs_review",
						rollback_state: "available",
						run_id: "run_legacy",
						source_command_id: "message_legacy",
						thread_id: "thread_1",
						updated_at: "2026-07-13T12:00:00.000Z",
						version: 1,
						workspace_id: "workspace_1",
					});
					const legacy = yield* ReadDiff("change_legacy").pipe(Effect.exit);

					return { legacy, missing };
				}),
			);

			expect(failure_from(result.missing)).toEqual(
				new WorkspaceChangeDiffUnavailable({ reason: "corrupt" }),
			);
			expect(failure_from(result.legacy)).toEqual(
				new WorkspaceChangeDiffUnavailable({ reason: "legacy_unavailable" }),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects duplicate committed validation after a stored artifact is corrupted", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "duplicate");

		try {
			const outcome = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					yield* CommitRecorded();
					const database = yield* Database;
					const repository = yield* WorkspaceChangeRepository;
					const prepared = yield* Prepare();

					yield* database.client.run(
						`UPDATE workspace_change_diffs SET patch = zeroblob(${prepared.patch.byteLength}) WHERE change_id = 'change_1'`,
					);

					return yield* repository
						.CommitRecorded("message_1", prepared)
						.pipe(Effect.exit);
				}),
			);

			expect(failure_from(outcome)).toMatchObject({ _tag: "JournalInvariantError" });
		} finally {
			await runtime.dispose();
		}
	});

	it("rolls back projection and journal state when diff insertion fails, then recovers the applied operation", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "rollback");

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					const repository = yield* WorkspaceChangeRepository;
					const prepared = yield* Prepare();

					yield* repository.ClaimReplace(replace_claim());
					yield* repository.MarkApplied({
						_tag: "replace",
						message_id: "message_1",
						result_identity: identity(text_encoder.encode(after)),
					});
					yield* InstallDiffInsertFailure;
					const failed = yield* repository
						.CommitRecorded("message_1", prepared)
						.pipe(Effect.exit);
					const rolled_back = yield* ReadRows;
					yield* RemoveDiffInsertFailure;
					const recovered = yield* repository.CommitRecorded("message_1", prepared);

					return { failed, recovered, rolled_back };
				}),
			);

			expect(failure_from(result.failed)).toMatchObject({
				_tag: "JournalStoreFailure",
			});
			expect(result.rolled_back).toMatchObject({
				changes: [],
				commands: [],
				diffs: [],
				events: [],
				operations: [expect.objectContaining({ lifecycle: "applied" })],
			});
			expect(result.recovered.status).toBe("accepted");
		} finally {
			await runtime.dispose();
		}
	});

	it("applies fresh migrations with foreign keys enforced", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "foreign-keys");

		try {
			const outcome = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					return yield* database.client
						.insert(WorkspaceChangeDiffs)
						.values({
							added_line_count: 0,
							after_identity_json: JSON.stringify(identity(new Uint8Array())),
							before_identity_json: JSON.stringify(identity(new Uint8Array())),
							change_id: "missing_change",
							context_lines: 3,
							created_at: "2026-07-13T12:00:00.000Z",
							format: "unified",
							format_version: 1,
							patch: Buffer.from(expected_patch),
							patch_byte_count: expected_patch.length,
							patch_hash: createHash("sha256").update(expected_patch).digest("hex"),
							path: "src/example.ts",
							removed_line_count: 0,
							source_command_id: "missing_message",
							thread_id: "thread_missing",
							workspace_id: "workspace_missing",
						})
						.pipe(Effect.exit);
				}),
			);

			expect(Exit.isFailure(outcome)).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("upgrades a committed legacy projection to an explicitly unavailable diff", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const prior_migrations_path = await Effect.runPromise(MakePriorMigrationsPath);
		const legacy_runtime = make_database_runtime(database_path, prior_migrations_path);

		try {
			await legacy_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* database.client.run(`
						INSERT INTO threads (
							thread_id, title, title_source, created_at, updated_at
						) VALUES (
							'thread_1', 'Legacy diff thread', 'initial',
							'2026-07-13T12:00:00.000Z', '2026-07-13T12:00:00.000Z'
						)
					`);
					yield* database.client.run(`
						INSERT INTO workspace_change_operations (
							message_id, action, request_fingerprint, change_id, thread_id,
							run_id, agent_id, raw_origin_json, workspace_id, path,
							expected_identity_json, result_identity_json, lifecycle,
							evidence_recorded, journal_sequence, sent_at, created_at, updated_at
						) VALUES (
							'message_legacy', 'replace', '${"a".repeat(64)}', 'change_legacy', 'thread_1',
							'run_legacy', 'agent_legacy', NULL, 'workspace_1', 'src/legacy.ts',
							'${JSON.stringify(identity(text_encoder.encode(before)))}',
							'${JSON.stringify(identity(text_encoder.encode(after)))}', 'committed',
							false, 1, '2026-07-13T12:00:00.000Z',
							'2026-07-13T12:00:00.000Z', '2026-07-13T12:00:00.000Z'
						)
					`);
					yield* database.client.run(`
						INSERT INTO workspace_changes (
							change_id, source_command_id, thread_id, workspace_id, path,
							before_identity_json, after_identity_json, run_id, agent_id,
							raw_origin_json, review_state, rollback_state, reviewed_at,
							rolled_back_at, version, created_at, updated_at
						) VALUES (
							'change_legacy', 'message_legacy', 'thread_1', 'workspace_1', 'src/legacy.ts',
							'${JSON.stringify(identity(text_encoder.encode(before)))}',
							'${JSON.stringify(identity(text_encoder.encode(after)))}',
							'run_legacy', 'agent_legacy', NULL, 'needs_review', 'available', NULL,
							NULL, 1, '2026-07-13T12:00:00.000Z', '2026-07-13T12:00:00.000Z'
						)
					`);
					yield* database.client.run(`
						INSERT INTO workspace_mutation_authorities (
							message_id, change_id, thread_id, run_id, agent_id,
							workspace_id, authority_kind, working_directory, created_at
						) VALUES (
							'message_legacy', 'change_legacy', 'thread_1', 'run_legacy',
							'agent_legacy', 'workspace_1', 'base_run', 'C:/work/legacy',
							'2026-07-13T12:00:00.000Z'
						)
					`);
					yield* database.client.run(`
						INSERT INTO workspace_mutation_payloads (
							message_id, thread_id, state, created_at, updated_at
						) VALUES (
							'message_legacy', 'thread_1', 'consumed',
							'2026-07-13T12:00:00.000Z', '2026-07-13T12:00:00.000Z'
						)
					`);
				}),
			);
		} finally {
			await legacy_runtime.dispose();
		}

		const upgraded_runtime = make_runtime(database_path, "upgrade");

		try {
			const result = await upgraded_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const authorities = yield* database.client
						.select()
						.from(WorkspaceMutationAuthorities);
					const changes = yield* database.client.select().from(WorkspaceChanges);
					const payloads = yield* database.client
						.select()
						.from(WorkspaceMutationPayloads);
					const all_diffs = yield* database.client.select().from(WorkspaceChangeDiffs);
					const authority = authorities.find(
						(row) => row.message_id === "message_legacy",
					);
					const change = changes.find((row) => row.change_id === "change_legacy");
					const diffs = all_diffs.filter((row) => row.change_id === "change_legacy");
					const read = yield* ReadDiff("change_legacy").pipe(Effect.exit);
					const invalid_operation_version = yield* database.client
						.run(`
							UPDATE workspace_change_operations
							SET diff_format_version = 2
							WHERE message_id = 'message_legacy'
						`)
						.pipe(Effect.exit);
					const invalid_diff_state = yield* database.client
						.run(`
							UPDATE workspace_changes
							SET diff_state = 'invalid'
							WHERE change_id = 'change_legacy'
						`)
						.pipe(Effect.exit);

					return {
						authority,
						change,
						diffs,
						invalid_diff_state,
						invalid_operation_version,
						payloads,
						read,
					};
				}),
			);

			expect(result.authority).toMatchObject({
				authority_kind: "base_run",
				change_id: "change_legacy",
				message_id: "message_legacy",
			});
			expect(result.change).toMatchObject({ diff_state: "legacy_unavailable" });
			expect(result.diffs).toEqual([]);
			expect(result.payloads).toEqual([
				expect.objectContaining({ message_id: "message_legacy", state: "consumed" }),
			]);
			expect(Exit.isFailure(result.invalid_operation_version)).toBe(true);
			expect(Exit.isFailure(result.invalid_diff_state)).toBe(true);
			expect(failure_from(result.read)).toEqual(
				new WorkspaceChangeDiffUnavailable({ reason: "legacy_unavailable" }),
			);
		} finally {
			await upgraded_runtime.dispose();
		}
	});
});
