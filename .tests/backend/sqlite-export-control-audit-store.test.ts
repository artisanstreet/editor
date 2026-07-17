import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { ExportControlAuditRecord, ExportControlDecision } from "@artisan/protocol";

import {
	ExportControlAuditConflict,
	ExportControlAuditFailure,
	ExportControlAuditStore,
	type ExportControlAuditCommit,
} from "../../modules/backend/src/compliance/export-control";
import { SQLiteExportControlAuditStoreLive } from "../../modules/backend/src/compliance/sqlite-export-control-audit-store";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { ExportControlAuditDecisions } from "../../modules/backend/src/persistence/schema";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-export-control-audit-",
	});

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function make_runtime(database_path: string) {
	const database = make_database_layer({ database_path, migrations_path });
	const store = SQLiteExportControlAuditStoreLive.pipe(Layer.provide(database));

	return ManagedRuntime.make(Layer.merge(database, store));
}

function commit(
	decision_id: string,
	intent_fingerprint = "a".repeat(64),
): ExportControlAuditCommit {
	const decision: ExportControlDecision = {
		decision: "allowed",
		decision_id,
		policy_id: "policy_export_1",
		policy_version: 1,
	};
	const record: ExportControlAuditRecord = {
		action: "release",
		decision: "allowed",
		decision_id,
		occurred_at: "2026-07-17T12:00:00.000Z",
		policy_id: "policy_export_1",
		policy_version: 1,
		reason_code: "allowed",
		signal_kinds: ["account_country"],
	};

	return { decision, intent_fingerprint, record };
}

afterEach(async () => {
	const cleanup = directories.splice(0);

	await Effect.runPromise(
		Effect.forEach(cleanup, (directory) =>
			FileSystem.FileSystem.pipe(
				Effect.flatMap((file_system) => file_system.remove(directory, { recursive: true })),
			),
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("SQLiteExportControlAuditStore", () => {
	it("exact-replays one privacy-bounded decision after restart", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const first_runtime = make_runtime(database_path);
		const input = commit("decision_restart");

		try {
			await first_runtime.runPromise(
				ExportControlAuditStore.pipe(Effect.flatMap((store) => store.Commit(input))),
			);
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_runtime(database_path);

		try {
			const result = await second_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const store = yield* ExportControlAuditStore;
					const replay = yield* store.Commit(input);
					const rows = yield* database.client.select().from(ExportControlAuditDecisions);

					return { replay, rows };
				}),
			);

			expect(result.replay).toEqual(input.decision);
			expect(result.rows).toHaveLength(1);
			expect(JSON.stringify(result.rows)).not.toMatch(/\b(?:NO|RU)\b/u);
			expect(JSON.stringify(result.rows)).toContain("account_country");
		} finally {
			await second_runtime.dispose();
		}
	});

	it("serializes same-intent commits across two runtimes", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const first_runtime = make_runtime(database_path);
		const second_runtime = make_runtime(database_path);
		const input = commit("decision_concurrent");

		try {
			await first_runtime.runPromise(Database.pipe(Effect.asVoid));
			await second_runtime.runPromise(Database.pipe(Effect.asVoid));

			const results = await Promise.all([
				first_runtime.runPromise(
					ExportControlAuditStore.pipe(Effect.flatMap((store) => store.Commit(input))),
				),
				second_runtime.runPromise(
					ExportControlAuditStore.pipe(Effect.flatMap((store) => store.Commit(input))),
				),
			]);
			const rows = await first_runtime.runPromise(
				Database.pipe(
					Effect.flatMap((database) =>
						database.client.select().from(ExportControlAuditDecisions),
					),
				),
			);

			expect(results).toEqual([input.decision, input.decision]);
			expect(rows).toHaveLength(1);
		} finally {
			await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
		}
	});

	it("rejects changed intent for an existing decision id", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path);
		const first = commit("decision_conflict", "a".repeat(64));
		const changed = commit("decision_conflict", "b".repeat(64));

		try {
			const failure = await runtime.runPromise(
				Effect.gen(function* () {
					const store = yield* ExportControlAuditStore;

					yield* store.Commit(first);

					return yield* Effect.flip(store.Commit(changed));
				}),
			);

			expect(failure).toBeInstanceOf(ExportControlAuditConflict);
		} finally {
			await runtime.dispose();
		}
	});

	it("fails closed when persisted decision JSON is corrupt", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path);
		const input = commit("decision_corrupt");

		try {
			const failure = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const store = yield* ExportControlAuditStore;

					yield* store.Commit(input);
					yield* database.client
						.update(ExportControlAuditDecisions)
						.set({ decision_json: "{}" });

					return yield* Effect.flip(store.Commit(input));
				}),
			);

			expect(failure).toBeInstanceOf(ExportControlAuditFailure);
		} finally {
			await runtime.dispose();
		}
	});

	it("fails closed when stored payloads are bound to a different decision id", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path);
		const input = commit("decision_misbound");
		const other = commit("decision_other");

		try {
			const failure = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const store = yield* ExportControlAuditStore;

					yield* store.Commit(input);
					yield* database.client.update(ExportControlAuditDecisions).set({
						decision_json: JSON.stringify(other.decision),
						record_json: JSON.stringify(other.record),
					});

					return yield* Effect.flip(store.Commit(input));
				}),
			);

			expect(failure).toBeInstanceOf(ExportControlAuditFailure);
		} finally {
			await runtime.dispose();
		}
	});

	it("fails closed when stored audit intent fields are coherently rewritten", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path);
		const input = commit("decision_rewritten_intent");
		const rewritten_record: ExportControlAuditRecord = {
			...input.record,
			action: "billing",
			signal_kinds: ["billing_country"],
		};

		try {
			const failure = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const store = yield* ExportControlAuditStore;

					yield* store.Commit(input);
					yield* database.client.update(ExportControlAuditDecisions).set({
						action: "billing",
						record_json: JSON.stringify(rewritten_record),
					});

					return yield* Effect.flip(store.Commit(input));
				}),
			);

			expect(failure).toBeInstanceOf(ExportControlAuditFailure);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects inconsistent decision and audit record before writing", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path);
		const input = commit("decision_inconsistent");
		const inconsistent: ExportControlAuditCommit = {
			...input,
			record: { ...input.record, decision: "restricted", reason_code: "restricted_region" },
		};

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const store = yield* ExportControlAuditStore;
					const failure = yield* Effect.flip(store.Commit(inconsistent));
					const rows = yield* database.client.select().from(ExportControlAuditDecisions);

					return { failure, rows };
				}),
			);

			expect(result.failure).toBeInstanceOf(ExportControlAuditFailure);
			expect(result.rows).toHaveLength(0);
		} finally {
			await runtime.dispose();
		}
	});
});
