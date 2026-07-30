import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	RoutineRepository,
	RoutineRepositoryLive,
} from "../../modules/backend/src/marketplace/routines/repository";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";
import { MarketplaceRoutines } from "../../modules/backend/src/persistence/tables";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];

const MakePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-routine-repository-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};

const MetadataLive = Layer.succeed(RuntimeMetadata, {
	instance_id: "routine_repository_test",
	MakeId: (prefix) => Effect.sync(() => `${prefix}_test`),
	Now: Effect.succeed("2026-07-18T12:00:00.000Z"),
});

const MakeRuntime = (database_path: string) =>
	ManagedRuntime.make(
		RoutineRepositoryLive.pipe(
			Layer.provideMerge(
				Layer.mergeAll(
					make_database_layer({ database_path, migrations_path }),
					JournalNotifierLive,
					MetadataLive,
				),
			),
		),
	);

const routine_detail = {
	compatibility: [{ engine_id: "codex", state: "native" as const }],
	description: "A deterministic routine",
	display_name: "Routine A",
	enabled: true,
	exported_commands: [{ description: "Run it", name: "routine-a" }],
	files: [{ path: "SKILL.md", required: true }],
	id: "routine_a",
	instructions: "Do the deterministic thing.",
	permissions: [{ description: "Read the workspace", kind: "filesystem_read" as const }],
	scope: { kind: "global" as const },
	status: "enabled" as const,
	source: { kind: "catalog" as const, locator: "routine-a" },
	sync: [],
	trust: "verified" as const,
	version: "1.0.0",
};

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("RoutineRepository", () => {
	it("persists an install request once without bypassing its decision", async () => {
		const runtime = MakeRuntime(await MakePath());
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* RoutineRepository;
					const operation = {
						approval_fingerprint: "preview_a",
						approval_id: "approval_a",
						operation_id: "operation_a",
						preview_json: "{}",
						request_fingerprint: "request_a",
						routine_id: "routine_a",
					};
					return [
						yield* repository.RecordPendingInstall(operation),
						yield* repository.RecordPendingInstall(operation),
					];
				}),
			);
			expect(result).toEqual(["accepted", "duplicate"]);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects a reused operation id with changed approval intent", async () => {
		const runtime = MakeRuntime(await MakePath());
		try {
			const failure = await runtime.runPromiseExit(
				Effect.gen(function* () {
					const repository = yield* RoutineRepository;
					yield* repository.RecordPendingInstall({
						approval_fingerprint: "preview_a",
						approval_id: "approval_a",
						operation_id: "operation_a",
						preview_json: "{}",
						request_fingerprint: "request_a",
						routine_id: "routine_a",
					});
					yield* repository.RecordPendingInstall({
						approval_fingerprint: "preview_b",
						approval_id: "approval_a",
						operation_id: "operation_a",
						preview_json: "{}",
						request_fingerprint: "request_b",
						routine_id: "routine_a",
					});
				}),
			);
			expect(failure._tag).toBe("Failure");
		} finally {
			await runtime.dispose();
		}
	});

	it("requires a durable pending decision before committing and decodes detail progressively", async () => {
		const runtime = MakeRuntime(await MakePath());
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* RoutineRepository;
					const operation = {
						approval_fingerprint: "preview_a",
						approval_id: "approval_a",
						operation_id: "operation_a",
						preview_json: "{}",
						request_fingerprint: "request_a",
						routine_id: "routine_a",
					};
					const pending = yield* repository.RecordPendingInstall(operation);
					const decision = yield* repository.DecideInstall({
						approval_fingerprint: "preview_a",
						approval_id: "approval_a",
						approved: true,
						operation_id: "operation_a",
					});
					yield* repository.CommitInstalled({
						artifact_refs: ["artifact_a"],
						detail: routine_detail,
						operation_id: "operation_a",
					});
					return {
						detail: yield* repository.ReadDetail("routine_a"),
						pending,
						decision,
						summaries: yield* repository.ReadSummaries,
					};
				}),
			);
			expect(result.pending).toBe("accepted");
			expect(result.decision).toBe("approved");
			expect(result.summaries).toHaveLength(1);
			expect(result.detail.instructions).toBe("Do the deterministic thing.");
		} finally {
			await runtime.dispose();
		}
	});

	it("persists denial without creating a routine", async () => {
		const runtime = MakeRuntime(await MakePath());
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* RoutineRepository;
					yield* repository.RecordPendingInstall({
						approval_fingerprint: "preview_a",
						approval_id: "approval_a",
						operation_id: "operation_a",
						preview_json: "{}",
						request_fingerprint: "request_a",
						routine_id: "routine_a",
					});
					const decision = yield* repository.DecideInstall({
						approval_fingerprint: "preview_a",
						approval_id: "approval_a",
						approved: false,
						operation_id: "operation_a",
					});
					return { decision, summaries: yield* repository.ReadSummaries };
				}),
			);
			expect(result).toEqual({ decision: "denied", summaries: [] });
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects corrupt persisted routine metadata instead of exposing unchecked detail", async () => {
		const runtime = MakeRuntime(await MakePath());
		try {
			const exit = await runtime.runPromiseExit(
				Effect.gen(function* () {
					const repository = yield* RoutineRepository;
					yield* repository.RecordPendingInstall({
						approval_fingerprint: "preview_corrupt",
						approval_id: "approval_corrupt",
						operation_id: "operation_corrupt",
						preview_json: "{}",
						request_fingerprint: "request_corrupt",
						routine_id: "routine_a",
					});
					yield* repository.DecideInstall({
						approval_fingerprint: "preview_corrupt",
						approval_id: "approval_corrupt",
						approved: true,
						operation_id: "operation_corrupt",
					});
					yield* repository.CommitInstalled({
						artifact_refs: [],
						detail: routine_detail,
						operation_id: "operation_corrupt",
					});
					const database = yield* Database;
					yield* database.client
						.update(MarketplaceRoutines)
						.set({ scope_json: "not-json" });
					yield* repository.ReadDetail("routine_a");
				}),
			);
			expect(exit._tag).toBe("Failure");
		} finally {
			await runtime.dispose();
		}
	});

	it("atomically leases a provider mirror effect until its canonical completion", async () => {
		const runtime = MakeRuntime(await MakePath());
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* RoutineRepository;
					const operation = {
						engine_id: "codex",
						intent_fingerprint: "sync_a",
						kind: "sync" as const,
						operation_id: "sync_a",
						routine_id: "routine_a",
					};
					yield* repository.RecordPendingInstall({
						approval_fingerprint: "preview_mirror",
						approval_id: "approval_mirror",
						operation_id: "install_mirror",
						preview_json: "{}",
						request_fingerprint: "request_mirror",
						routine_id: "routine_a",
					});
					yield* repository.DecideInstall({
						approval_fingerprint: "preview_mirror",
						approval_id: "approval_mirror",
						approved: true,
						operation_id: "install_mirror",
					});
					yield* repository.CommitInstalled({
						artifact_refs: [],
						detail: routine_detail,
						operation_id: "install_mirror",
					});
					const first = yield* repository.ClaimMirrorOperation(operation);
					const duplicate = yield* repository.ClaimMirrorOperation(operation);
					const completed = yield* repository.CommitMirrorOperation({
						operation_id: operation.operation_id,
						state: {
							engine_id: operation.engine_id,
							status: "synced",
							updated_at: "2026-07-18T12:00:00.000Z",
						},
					});
					const retry = yield* repository.ClaimMirrorOperation(operation);
					return { completed, duplicate, first, retry };
				}),
			);
			expect(result.first).toEqual({ _tag: "Claimed" });
			expect(result.duplicate).toEqual({ _tag: "InFlight" });
			expect(result.completed).toBeGreaterThan(0);
			expect(result.retry).toMatchObject({ _tag: "Completed" });
		} finally {
			await runtime.dispose();
		}
	});
});
