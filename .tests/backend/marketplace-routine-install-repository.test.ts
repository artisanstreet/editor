import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	RoutineInstallConflict,
	RoutineInstallInvariant,
	RoutineInstallRepository,
	RoutineInstallRepositoryLive,
} from "../../modules/backend/src/marketplace/routine-install-repository";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	routine_install_identity_json_maximum_bytes,
	routine_install_json_maximum_bytes,
	RoutineInstallApprovals,
	RoutineInstallationHistory,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const routine_install_migration = "20260717150052_rare_gertrude_yorkes";
const directories: Array<string> = [];
const runtimes: Array<ManagedRuntime.ManagedRuntime<any, any>> = [];
const now = "2026-07-17T12:00:00.000Z";
const instructions_sentinel = "ROUTINE-INSTRUCTIONS-MUST-NEVER-REACH-SQLITE";
const text_encoder = new TextEncoder();

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({ prefix: "artisan-routine-install-" });

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

const MakeMigrationPaths = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-routine-install-migration-",
	});
	const prior_migrations_path = join(directory, "prior-drizzle");
	const database_path = join(directory, "artisan.db");
	const entries = yield* file_system.readDirectory(migrations_path);
	const prior_entries = entries.filter((entry) => entry < routine_install_migration);

	directories.push(directory);
	yield* file_system.makeDirectory(prior_migrations_path, { recursive: true });
	yield* Effect.forEach(
		prior_entries,
		(entry) =>
			file_system.copy(join(migrations_path, entry), join(prior_migrations_path, entry)),
		{ concurrency: "unbounded" },
	);

	return { database_path, prior_migrations_path };
}).pipe(Effect.provide(NodeFileSystem.layer));

function make_runtime(
	database_path: string,
	instance_id: string,
	timestamps: ReadonlyArray<string> = [now],
) {
	let identifier = 0;
	let timestamp_index = 0;
	const metadata = Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${instance_id}_${++identifier}`),
		Now: Effect.sync(() => timestamps[timestamp_index++] ?? timestamps.at(-1) ?? now),
	});
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		metadata,
		NodeCrypto.layer,
	);

	const runtime = ManagedRuntime.make(
		RoutineInstallRepositoryLive.pipe(Layer.provideMerge(infrastructure)),
	);

	runtimes.push(runtime);

	return runtime;
}

function preview(
	overrides: {
		readonly candidate?: Readonly<Record<string, unknown>>;
		readonly installation_id?: string;
		readonly preview_operation_id?: string;
		readonly scope?: Readonly<Record<string, unknown>>;
	} = {},
) {
	const scope = overrides.scope ?? { kind: "global" as const };
	const identity = {
		source: {
			display_name: "Artisan catalog",
			kind: "catalog" as const,
			locator: "artisan.release-notes",
		},
		version: "1.2.3",
	};
	const candidate = {
		commands: [
			{
				command_id: "publish",
				description: "Publishes release notes.",
				label: "Publish",
			},
		],
		compatibility: ["codex", "claude"],
		display_name: "Release notes",
		files: [
			{
				path: "ROUTINE.md",
				purpose: "Routine instructions.",
				write_mode: "create" as const,
			},
		],
		instructions: { content_hash: "a".repeat(64) },
		permissions: [
			{
				kind: "filesystem_write" as const,
				label: "Write release notes",
				required: true,
			},
		],
		scope,
		summary: {
			description: "Prepares and publishes release notes.",
			display_name: "Release notes",
			identity,
			routine_id: "routine.release-notes",
		},
		trust: { level: "verified" as const, reasons: ["Catalog review completed."] },
		...overrides.candidate,
	};

	return {
		candidate,
		preview_operation_id: overrides.preview_operation_id ?? "preview_1",
		rollback: {
			actions: ["Remove the installed Routine file."],
			available: true,
			identity: candidate.summary.identity,
			installation_id: overrides.installation_id ?? "installation_1",
			plan_fingerprint: "b".repeat(64),
			plan_version: 1,
			rollback_id: "rollback_1",
			scope: candidate.scope,
		},
	};
}

function boundary_preview(length_semantics: "effect" | "unicode_scalar" = "effect") {
	const item_count = 128;
	const astral = "\u{1f600}";
	const boundary_text = (length: number, suffix = "") =>
		length_semantics === "unicode_scalar"
			? astral.repeat(length - suffix.length) + suffix
			: astral + "\u0800".repeat(length - astral.length - suffix.length) + suffix;
	const indexed_boundary_text = (length: number, index: number) =>
		boundary_text(length, String(index).padStart(4, "0"));
	const indexed_ascii_text = (fill: string, length: number, index: number) =>
		fill.repeat(length - 4) + String(index).padStart(4, "0");
	const source_prefix = "https://example.com/";
	const identity = {
		source: {
			display_name: boundary_text(256),
			kind: "git" as const,
			locator: source_prefix + boundary_text(2_048 - source_prefix.length),
		},
		version: "1.0.0+" + "a".repeat(122),
	};
	const display_name = boundary_text(256);
	const scope = { kind: "workspace" as const, workspace_id: "w".repeat(256) };
	const permission_kinds = [
		"filesystem_read",
		"filesystem_write",
		"process_start",
		"network_connect",
		"browser_access",
		"account_access",
		"secret_reference",
	] as const;

	return {
		candidate: {
			commands: Array.from({ length: item_count }, (_, index) => ({
				command_id: indexed_ascii_text("c", 256, index),
				description: indexed_boundary_text(2_048, index),
				label: boundary_text(256),
			})),
			compatibility: ["codex", "claude"] as const,
			display_name,
			files: Array.from({ length: item_count }, (_, index) => ({
				path: indexed_ascii_text("p", 1_024, index),
				purpose: indexed_boundary_text(2_048, index),
				write_mode: "create" as const,
			})),
			instructions: { content_hash: "a".repeat(64) },
			permissions: permission_kinds.map((kind) => ({
				kind,
				label: boundary_text(256),
				required: true,
			})),
			scope,
			summary: {
				description: boundary_text(2_048),
				display_name,
				identity,
				routine_id: "r".repeat(256),
			},
			trust: {
				level: "verified" as const,
				reasons: Array.from({ length: item_count }, (_, index) =>
					indexed_boundary_text(2_048, index),
				),
			},
		},
		preview_operation_id: "p".repeat(256),
		rollback: {
			actions: Array.from({ length: item_count }, (_, index) =>
				indexed_boundary_text(2_048, index),
			),
			available: true,
			identity,
			installation_id: "i".repeat(256),
			plan_fingerprint: "b".repeat(64),
			plan_version: 1,
			rollback_id: "b".repeat(256),
			scope,
		},
	};
}

function run<A>(
	runtime: ManagedRuntime.ManagedRuntime<any, any>,
	effect: (repository: typeof RoutineInstallRepository.Service) => Effect.Effect<A, unknown>,
) {
	return runtime.runPromise(Effect.flatMap(RoutineInstallRepository, effect));
}

function decision(approval: unknown, overrides: Readonly<Record<string, unknown>> = {}) {
	return {
		approval,
		approval_id: "approval_first_1",
		decision: "approved" as const,
		decision_id: "decision_1",
		operation_id: "decision_operation_1",
		preview_operation_id: "preview_1",
		...overrides,
	};
}

function install(approval: unknown, overrides: Readonly<Record<string, unknown>> = {}) {
	return {
		approval,
		approval_id: "approval_first_1",
		installation_id: "installation_1",
		operation_id: "install_1",
		preview_operation_id: "preview_1",
		scope: { kind: "global" as const },
		...overrides,
	};
}

function expect_conflict(error: unknown) {
	return error instanceof RoutineInstallConflict;
}

afterEach(async () => {
	const cleanup = directories.splice(0);
	const active_runtimes = runtimes.splice(0);

	await Promise.all(active_runtimes.map((runtime) => runtime.dispose()));

	await Effect.runPromise(
		Effect.forEach(cleanup, (directory) =>
			Effect.flatMap(FileSystem.FileSystem, (file_system) =>
				file_system.remove(directory, { force: true, recursive: true }),
			),
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("RoutineInstallRepository", () => {
	it("preserves pre-existing database rows while applying the Routine install migration", async () => {
		const paths = await Effect.runPromise(MakeMigrationPaths);
		const prior_runtime = ManagedRuntime.make(
			make_database_layer({
				database_path: paths.database_path,
				migrations_path: paths.prior_migrations_path,
			}),
		);

		try {
			await prior_runtime.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client.run(`
						INSERT INTO threads (thread_id, title, created_at, updated_at)
						VALUES ('thread_before_routine_install', 'Legacy thread', '${now}', '${now}')
					`),
				),
			);
		} finally {
			await prior_runtime.dispose();
		}

		const current_runtime = ManagedRuntime.make(
			make_database_layer({ database_path: paths.database_path, migrations_path }),
		);

		try {
			const result = await current_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					return {
						approvals: yield* database.client.select().from(RoutineInstallApprovals),
						installations: yield* database.client
							.select()
							.from(RoutineInstallationHistory),
						threads: yield* database.client.select().from(Threads),
					};
				}),
			);

			expect(result.approvals).toEqual([]);
			expect(result.installations).toEqual([]);
			expect(result.threads).toEqual([
				expect.objectContaining({
					thread_id: "thread_before_routine_install",
					title: "Legacy thread",
				}),
			]);
		} finally {
			await current_runtime.dispose();
		}
	});

	it("replays an exact preview and rejects every material candidate substitution", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "first");
		const original = preview();
		const accepted = await run(runtime, (repository) => repository.Preview(original));

		await expect(run(runtime, (repository) => repository.Preview(original))).resolves.toEqual(
			accepted,
		);

		for (const candidate of [
			{
				...original.candidate,
				commands: [{ ...original.candidate.commands[0], label: "Changed" }],
			},
			{ ...original.candidate, compatibility: ["codex"] },
			{
				...original.candidate,
				files: [{ ...original.candidate.files[0], purpose: "Changed" }],
			},
			{ ...original.candidate, instructions: { content_hash: "c".repeat(64) } },
			{
				...original.candidate,
				permissions: [{ ...original.candidate.permissions[0], label: "Changed" }],
			},
			{ ...original.candidate, trust: { level: "unverified", reasons: [] } },
			{
				...original.candidate,
				display_name: "Changed name",
				summary: { ...original.candidate.summary, display_name: "Changed name" },
			},
			{
				...original.candidate,
				summary: {
					...original.candidate.summary,
					identity: { ...original.candidate.summary.identity, version: "1.2.4" },
				},
			},
		]) {
			await expect(
				run(runtime, (repository) =>
					repository.Preview({
						...original,
						candidate,
						rollback: { ...original.rollback, identity: candidate.summary.identity },
					}),
				),
			).rejects.toSatisfy(expect_conflict);
		}

		await expect(
			run(runtime, (repository) =>
				repository.Preview({
					...original,
					candidate: {
						...original.candidate,
						scope: { kind: "workspace", workspace_id: "workspace_1" },
					},
					rollback: {
						...original.rollback,
						scope: { kind: "workspace", workspace_id: "workspace_1" },
					},
				}),
			),
		).rejects.toSatisfy(expect_conflict);
	});

	it("persists exact decisions, makes denial terminal, and settles only an approved snapshot", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "first");
		const pending = await run(runtime, (repository) => repository.Preview(preview()));

		await expect(
			run(runtime, (repository) => repository.Install(install(pending))),
		).rejects.toSatisfy(expect_conflict);

		const approved = await run(runtime, (repository) => repository.Decide(decision(pending)));

		await expect(
			run(runtime, (repository) => repository.Decide(decision(pending))),
		).resolves.toEqual(approved);
		await expect(
			run(runtime, (repository) =>
				repository.Decide(decision(pending, { decision: "denied" })),
			),
		).rejects.toSatisfy(expect_conflict);
		await expect(
			run(runtime, (repository) =>
				repository.Install(
					install({ ...approved, preview: preview({ preview_operation_id: "other" }) }),
				),
			),
		).rejects.toSatisfy(expect_conflict);

		const denied_database = await Effect.runPromise(MakeDatabasePath);
		const denied_runtime = make_runtime(denied_database, "denied");
		const denied_pending = await run(denied_runtime, (repository) =>
			repository.Preview(preview()),
		);
		const denied = await run(denied_runtime, (repository) =>
			repository.Decide(
				decision(denied_pending, {
					approval_id: denied_pending.approval_id,
					decision: "denied",
				}),
			),
		);

		await expect(
			run(denied_runtime, (repository) => repository.Install(install(denied))),
		).rejects.toSatisfy(expect_conflict);
	});

	it("fails closed when exact decision replay finds a forged terminal snapshot", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "first");
		const pending = await run(runtime, (repository) => repository.Preview(preview()));
		const request = decision(pending);

		await run(runtime, (repository) => repository.Decide(request));
		const storage = ManagedRuntime.make(
			make_database_layer({ database_path, migrations_path }),
		);

		await storage.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const [row] = yield* database.client.select().from(RoutineInstallApprovals);

				if (row?.decision_snapshot_json === null || row === undefined) {
					throw new Error("Expected one decided Routine approval");
				}

				const forged = JSON.parse(row.decision_snapshot_json) as {
					preview: {
						candidate: {
							commands: Array<{ label: string }>;
						};
					};
				};

				forged.preview.candidate.commands[0]!.label = "Forged but schema-valid label";
				yield* database.client
					.update(RoutineInstallApprovals)
					.set({ decision_snapshot_json: JSON.stringify(forged) });
			}),
		);

		await storage.dispose();
		await expect(
			run(runtime, (repository) => repository.Decide(request)),
		).rejects.toBeInstanceOf(RoutineInstallInvariant);
	});

	it("pins the exact decision timestamp after installation settlement", async () => {
		const created_at = "2026-07-17T12:00:00.000Z";
		const decided_at = "2026-07-17T12:10:00.000Z";
		const forged_decided_at = "2026-07-17T12:20:00.000Z";
		const installed_at = "2026-07-17T12:30:00.000Z";
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "timed", [
			created_at,
			decided_at,
			installed_at,
		]);
		const pending = await run(runtime, (repository) => repository.Preview(preview()));
		const decision_request = decision(pending, { approval_id: pending.approval_id });
		const approved = await run(runtime, (repository) => repository.Decide(decision_request));
		const install_request = install(approved, { approval_id: pending.approval_id });
		const installed = await run(runtime, (repository) => repository.Install(install_request));
		const storage = ManagedRuntime.make(
			make_database_layer({ database_path, migrations_path }),
		);
		const persisted_timestamps = await storage.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const [row] = yield* database.client.select().from(RoutineInstallApprovals);

				if (row?.decision_snapshot_json === null || row === undefined) {
					throw new Error("Expected one applied Routine approval");
				}

				const forged = JSON.parse(row.decision_snapshot_json) as { updated_at: string };
				const decision_snapshot_updated_at = forged.updated_at;

				forged.updated_at = forged_decided_at;
				yield* database.client
					.update(RoutineInstallApprovals)
					.set({ decision_snapshot_json: JSON.stringify(forged) });

				return {
					decided_at: row.decided_at,
					decision_snapshot_updated_at,
					updated_at: row.updated_at,
				};
			}),
		);

		await storage.dispose();
		expect(persisted_timestamps).toEqual({
			decided_at,
			decision_snapshot_updated_at: decided_at,
			updated_at: installed_at,
		});
		const read_query = {
			context: { engine: "codex" as const, scope: { kind: "global" as const } },
			routine: {
				identity: installed.routine.summary.identity,
				installation_id: installed.installation_id,
				routine_id: installed.routine.summary.routine_id,
				scope: { kind: "global" as const },
			},
		};

		await expect(
			run(runtime, (repository) => repository.Decide(decision_request)),
		).rejects.toBeInstanceOf(RoutineInstallInvariant);
		await expect(
			run(runtime, (repository) => repository.Get(read_query)),
		).rejects.toBeInstanceOf(RoutineInstallInvariant);
		await expect(
			run(runtime, (repository) =>
				repository.List({
					context: { engine: "codex", scope: { kind: "global" } },
				}),
			),
		).rejects.toBeInstanceOf(RoutineInstallInvariant);
		await expect(
			run(runtime, (repository) => repository.Install(install_request)),
		).rejects.toBeInstanceOf(RoutineInstallInvariant);
	});

	it("atomically installs the approved candidate, preserves scope separation, and reads after restart", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const first = make_runtime(database_path, "first");
		const pending = await run(first, (repository) => repository.Preview(preview()));
		const approved = await run(first, (repository) => repository.Decide(decision(pending)));
		const installed = await run(first, (repository) => repository.Install(install(approved)));

		expect(installed.routine.lifecycle).toBe("enabled");
		expect(installed.routine.sync).toEqual([
			{
				drift: "none",
				engine: "codex",
				identity: installed.routine.summary.identity,
				status: "runtime_only",
				updated_at: now,
			},
			{
				drift: "none",
				engine: "claude",
				identity: installed.routine.summary.identity,
				status: "runtime_only",
				updated_at: now,
			},
		]);

		await expect(
			run(first, (repository) => repository.Install(install(approved))),
		).resolves.toEqual(installed);
		await expect(
			run(first, (repository) =>
				repository.Install(
					install({
						...approved,
						preview: {
							...approved.preview,
							candidate: {
								...approved.preview.candidate,
								commands: [
									{
										...approved.preview.candidate.commands[0],
										label: "Changed after installation",
									},
								],
							},
						},
					}),
				),
			),
		).rejects.toSatisfy(expect_conflict);
		const second = make_runtime(database_path, "second");
		const listed = await run(second, (repository) =>
			repository.List({ context: { engine: "codex", scope: { kind: "global" } } }),
		);

		expect(listed.routines).toHaveLength(1);
		await expect(
			run(second, (repository) =>
				repository.Get({
					context: { engine: "codex", scope: { kind: "global" } },
					routine: {
						identity: installed.routine.summary.identity,
						installation_id: installed.installation_id,
						routine_id: installed.routine.summary.routine_id,
						scope: { kind: "global" },
					},
				}),
			),
		).resolves.toMatchObject({ routine: { installation: installed } });
		const workspace_preview = preview({
			installation_id: "installation_workspace_1",
			preview_operation_id: "preview_workspace_1",
			scope: { kind: "workspace", workspace_id: "workspace_1" },
		});
		const workspace_pending = await run(second, (repository) =>
			repository.Preview(workspace_preview),
		);
		const workspace_approved = await run(second, (repository) =>
			repository.Decide(
				decision(workspace_pending, {
					approval_id: workspace_pending.approval_id,
					decision_id: "decision_workspace_1",
					operation_id: "decision_operation_workspace_1",
					preview_operation_id: "preview_workspace_1",
				}),
			),
		);

		await run(second, (repository) =>
			repository.Install(
				install(workspace_approved, {
					approval_id: workspace_pending.approval_id,
					installation_id: "installation_workspace_1",
					operation_id: "install_workspace_1",
					preview_operation_id: "preview_workspace_1",
					scope: { kind: "workspace", workspace_id: "workspace_1" },
				}),
			),
		);

		await expect(
			run(second, (repository) =>
				repository.List({
					context: {
						engine: "codex",
						scope: { kind: "workspace", workspace_id: "workspace_1" },
					},
				}),
			),
		).resolves.toMatchObject({
			routines: [{ installation: { installation_id: "installation_workspace_1" } }],
		});
	});

	it("omits inactive installation history from List and Get", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "inactive");
		const pending = await run(runtime, (repository) => repository.Preview(preview()));
		const approved = await run(runtime, (repository) =>
			repository.Decide(decision(pending, { approval_id: pending.approval_id })),
		);
		const installed = await run(runtime, (repository) =>
			repository.Install(install(approved, { approval_id: pending.approval_id })),
		);
		const list_query = {
			context: { engine: "codex" as const, scope: { kind: "global" as const } },
		};
		const read_query = {
			context: { engine: "codex" as const, scope: { kind: "global" as const } },
			routine: {
				identity: installed.routine.summary.identity,
				installation_id: installed.installation_id,
				routine_id: installed.routine.summary.routine_id,
				scope: { kind: "global" as const },
			},
		};
		const storage = ManagedRuntime.make(
			make_database_layer({ database_path, migrations_path }),
		);

		await storage.runPromise(
			Effect.flatMap(Database, (database) =>
				database.client.update(RoutineInstallationHistory).set({ is_active: false }),
			),
		);

		await storage.dispose();
		await expect(run(runtime, (repository) => repository.List(list_query))).resolves.toEqual({
			query: list_query,
			routines: [],
		});
		await expect(run(runtime, (repository) => repository.Get(read_query))).resolves.toEqual({
			query: read_query,
		});
	});

	it("converges two live repository layers and rejects a second active slot", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const first = make_runtime(database_path, "first");
		const source = preview();
		const first_pending = await run(first, (repository) => repository.Preview(source));
		const second = make_runtime(database_path, "second");
		const second_pending = await run(second, (repository) => repository.Preview(source));

		expect(first_pending).toEqual(second_pending);
		const approved = await run(first, (repository) =>
			repository.Decide(decision(first_pending)),
		);
		const request = install(approved);
		const [first_install, second_install] = await Promise.all([
			run(first, (repository) => repository.Install(request)),
			run(second, (repository) => repository.Install(request)),
		]);

		expect(first_install).toEqual(second_install);
		const second_preview = preview({
			installation_id: "installation_2",
			preview_operation_id: "preview_2",
		});
		const second_pending_install = await run(first, (repository) =>
			repository.Preview(second_preview),
		);
		const second_approved = await run(first, (repository) =>
			repository.Decide(
				decision(second_pending_install, {
					approval_id: second_pending_install.approval_id,
					decision_id: "decision_2",
					operation_id: "decision_operation_2",
					preview_operation_id: "preview_2",
				}),
			),
		);
		const second_request = install(second_approved, {
			approval_id: second_pending_install.approval_id,
			installation_id: "installation_2",
			operation_id: "install_2",
			preview_operation_id: "preview_2",
		});

		await expect(
			run(first, (repository) => repository.Install(second_request)),
		).rejects.toSatisfy(expect_conflict);
		const storage = ManagedRuntime.make(
			make_database_layer({ database_path, migrations_path }),
		);
		const invalid_active = await storage.runPromise(
			Effect.flatMap(Database, (database) =>
				database.client.run("UPDATE routine_installation_history SET is_active = 2"),
			).pipe(Effect.exit),
		);

		expect(invalid_active._tag).toBe("Failure");
		await expect(
			run(first, (repository) => repository.Install(second_request)),
		).rejects.toMatchObject({ reason: "slot_conflict" });

		await storage.runPromise(
			Effect.flatMap(Database, (database) =>
				database.client.update(RoutineInstallationHistory).set({ is_active: false }),
			),
		);

		await storage.dispose();
		await expect(
			run(first, (repository) => repository.Install(second_request)),
		).resolves.toMatchObject({ installation_id: "installation_2" });
		const history_storage = ManagedRuntime.make(
			make_database_layer({ database_path, migrations_path }),
		);
		const history = await history_storage.runPromise(
			Effect.flatMap(Database, (database) =>
				database.client
					.select({ install_version: RoutineInstallationHistory.install_version })
					.from(RoutineInstallationHistory),
			),
		);

		await history_storage.dispose();
		expect(history.map((row) => row.install_version).toSorted()).toEqual([1, 2]);
	});

	it("converges divergent concurrent active-slot installs to one stable conflict", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const first = make_runtime(database_path, "race_first");
		const first_source = preview({
			installation_id: "installation_race_first",
			preview_operation_id: "preview_race_first",
		});
		const first_pending = await run(first, (repository) => repository.Preview(first_source));
		const second = make_runtime(database_path, "race_second");
		const second_source = preview({
			installation_id: "installation_race_second",
			preview_operation_id: "preview_race_second",
		});
		const second_pending = await run(second, (repository) => repository.Preview(second_source));
		const first_approved = await run(first, (repository) =>
			repository.Decide(
				decision(first_pending, {
					approval_id: first_pending.approval_id,
					decision_id: "decision_race_first",
					operation_id: "decision_operation_race_first",
					preview_operation_id: first_source.preview_operation_id,
				}),
			),
		);
		const second_approved = await run(second, (repository) =>
			repository.Decide(
				decision(second_pending, {
					approval_id: second_pending.approval_id,
					decision_id: "decision_race_second",
					operation_id: "decision_operation_race_second",
					preview_operation_id: second_source.preview_operation_id,
				}),
			),
		);
		const outcomes = await Promise.allSettled([
			run(first, (repository) =>
				repository.Install(
					install(first_approved, {
						approval_id: first_pending.approval_id,
						installation_id: first_source.rollback.installation_id,
						operation_id: "install_race_first",
						preview_operation_id: first_source.preview_operation_id,
					}),
				),
			),
			run(second, (repository) =>
				repository.Install(
					install(second_approved, {
						approval_id: second_pending.approval_id,
						installation_id: second_source.rollback.installation_id,
						operation_id: "install_race_second",
						preview_operation_id: second_source.preview_operation_id,
					}),
				),
			),
		]);
		const rejected = outcomes.find(
			(outcome): outcome is PromiseRejectedResult => outcome.status === "rejected",
		);

		expect(outcomes.filter((outcome) => outcome.status === "fulfilled")).toHaveLength(1);
		expect(rejected?.reason).toBeInstanceOf(RoutineInstallConflict);
		expect(rejected?.reason).toMatchObject({ reason: "slot_conflict" });
	});

	it("persists accepted boundaries and covers four-byte scalar projections", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "boundary");
		const source = boundary_preview();
		const pending = await run(runtime, (repository) => repository.Preview(source));
		const decision_request = decision(pending, {
			approval_id: pending.approval_id,
			decision_id: "d".repeat(256),
			operation_id: "o".repeat(256),
			preview_operation_id: source.preview_operation_id,
		});
		const approved = await run(runtime, (repository) => repository.Decide(decision_request));
		const installed = await run(runtime, (repository) =>
			repository.Install(
				install(approved, {
					approval_id: pending.approval_id,
					installation_id: source.rollback.installation_id,
					operation_id: "x".repeat(256),
					preview_operation_id: source.preview_operation_id,
					scope: source.candidate.scope,
				}),
			),
		);
		const json_projection_sizes = [
			source,
			pending,
			decision_request,
			approved,
			installed.routine,
		].map((projection) => text_encoder.encode(JSON.stringify(projection)).byteLength);
		const decision_request_size = text_encoder.encode(
			JSON.stringify(decision_request),
		).byteLength;
		const routine_size = text_encoder.encode(JSON.stringify(installed.routine)).byteLength;
		const identity_size = text_encoder.encode(
			JSON.stringify(source.candidate.summary.identity),
		).byteLength;
		const scalar_source = boundary_preview("unicode_scalar");
		const scalar_pending = { ...pending, preview: scalar_source };
		const scalar_decision_request = { ...decision_request, approval: scalar_pending };
		const scalar_routine = {
			...scalar_source.candidate,
			lifecycle: "enabled" as const,
			sync: scalar_source.candidate.compatibility.map((engine) => ({
				drift: "none" as const,
				engine,
				identity: scalar_source.candidate.summary.identity,
				status: "runtime_only" as const,
				updated_at: now,
			})),
			updated_at: now,
		};
		const scalar_projection_sizes = {
			decision_request: text_encoder.encode(JSON.stringify(scalar_decision_request))
				.byteLength,
			identity: text_encoder.encode(JSON.stringify(scalar_source.candidate.summary.identity))
				.byteLength,
			routine: text_encoder.encode(JSON.stringify(scalar_routine)).byteLength,
		};

		expect(text_encoder.encode("\u{1f600}").byteLength).toBe(4);
		expect(decision_request_size).toBeGreaterThan(3_400_000);
		expect(decision_request_size).toBeLessThan(3_500_000);
		expect(routine_size).toBeGreaterThan(2_600_000);
		expect(routine_size).toBeLessThan(2_700_000);
		expect(identity_size).toBeGreaterThan(7_000);
		expect(identity_size).toBeLessThan(7_100);
		expect(
			json_projection_sizes.every((size) => size <= routine_install_json_maximum_bytes),
		).toBe(true);
		expect(identity_size).toBeLessThanOrEqual(routine_install_identity_json_maximum_bytes);
		expect(scalar_projection_sizes.decision_request).toBeGreaterThan(4_500_000);
		expect(scalar_projection_sizes.decision_request).toBeLessThan(4_600_000);
		expect(scalar_projection_sizes.routine).toBeGreaterThan(3_450_000);
		expect(scalar_projection_sizes.routine).toBeLessThan(3_550_000);
		expect(scalar_projection_sizes.identity).toBeGreaterThan(9_300);
		expect(scalar_projection_sizes.identity).toBeLessThan(9_500);
		expect(scalar_projection_sizes.decision_request).toBeLessThanOrEqual(
			routine_install_json_maximum_bytes,
		);
		expect(scalar_projection_sizes.routine).toBeLessThanOrEqual(
			routine_install_json_maximum_bytes,
		);
		expect(scalar_projection_sizes.identity).toBeLessThanOrEqual(
			routine_install_identity_json_maximum_bytes,
		);
		expect(installed.routine.commands).toHaveLength(128);
		expect(installed.routine.files).toHaveLength(128);
		expect(installed.routine.trust.reasons).toHaveLength(128);
	});

	it("fails source-safely for corrupt storage and never persists instruction content", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, "first");
		const malformed_preview = preview();

		await expect(
			run(runtime, (repository) =>
				repository.Preview({
					...malformed_preview,
					candidate: {
						...malformed_preview.candidate,
						instructions: {
							...malformed_preview.candidate.instructions,
							content: instructions_sentinel,
						},
					},
				}),
			),
		).rejects.toSatisfy(expect_conflict);
		const pending = await run(runtime, (repository) => repository.Preview(preview()));
		const approved = await run(runtime, (repository) => repository.Decide(decision(pending)));
		const installed = await run(runtime, (repository) => repository.Install(install(approved)));
		const storage = ManagedRuntime.make(
			make_database_layer({ database_path, migrations_path }),
		);
		const original_decision_snapshot = await storage.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const [approval_row] = yield* database.client
					.select()
					.from(RoutineInstallApprovals);

				if (approval_row?.decision_snapshot_json === null || approval_row === undefined) {
					throw new Error("Expected one decided Routine approval");
				}

				const forged = JSON.parse(approval_row.decision_snapshot_json) as {
					preview: {
						candidate: {
							commands: Array<{ label: string }>;
						};
					};
				};

				forged.preview.candidate.commands[0]!.label = "Valid-looking forged command";
				yield* database.client
					.update(RoutineInstallApprovals)
					.set({ decision_snapshot_json: JSON.stringify(forged) });

				return approval_row.decision_snapshot_json;
			}),
		);

		await expect(
			run(runtime, (repository) =>
				repository.Get({
					context: { engine: "codex", scope: { kind: "global" } },
					routine: {
						identity: installed.routine.summary.identity,
						installation_id: installed.installation_id,
						routine_id: installed.routine.summary.routine_id,
						scope: { kind: "global" },
					},
				}),
			),
		).rejects.toBeInstanceOf(RoutineInstallInvariant);

		await storage.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client
					.update(RoutineInstallApprovals)
					.set({ decision_snapshot_json: original_decision_snapshot });
				yield* database.client
					.update(RoutineInstallationHistory)
					.set({ rollback_id: "valid_looking_forged_rollback" });
			}),
		);

		await expect(
			run(runtime, (repository) =>
				repository.List({
					context: { engine: "codex", scope: { kind: "global" } },
				}),
			),
		).rejects.toBeInstanceOf(RoutineInstallInvariant);

		await storage.dispose();
		const fresh_storage = ManagedRuntime.make(
			make_database_layer({ database_path, migrations_path }),
		);
		const stored = await fresh_storage.runPromise(
			Effect.flatMap(Database, (database) =>
				Effect.all([
					database.client.select().from(RoutineInstallApprovals),
					database.client.select().from(RoutineInstallationHistory),
				]),
			),
		);

		await fresh_storage.dispose();
		expect(JSON.stringify(stored)).not.toContain(instructions_sentinel);
	});
});
