import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";
import type { RoutineSource } from "@artisan/protocol";

import {
	RoutineInstaller,
	RoutineInstallerError,
	type RoutineInspection,
	RoutineMirrorRegistry,
	NpxSkillsAdapter,
	RoutineSourceInspector,
} from "../../modules/backend/src/marketplace/routines/routine-adapters";
import {
	RoutineRepository,
	RoutineRepositoryLive,
} from "../../modules/backend/src/marketplace/routines/routine-repository";
import {
	RoutineService,
	RoutineServiceLive,
} from "../../modules/backend/src/marketplace/routines/routine-service";
import { make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const rollback_operation_ids = new WeakMap<Array<string>, Set<string>>();
const inspection = {
	artifact_refs: ["artifact_a"],
	candidate_id: "routine_a",
	compatibility: [{ engine_id: "codex", state: "native" as const }],
	content_hashes: { "SKILL.md": "hash_a" },
	description: "A deterministic routine",
	display_name: "Routine A",
	exported_commands: [{ description: "Run it", name: "routine-a" }],
	files: [{ path: "SKILL.md", required: true }],
	instructions: "Do the deterministic thing.",
	permissions: [{ description: "Read workspace", kind: "filesystem_read" as const }],
	rollback_available: true,
	source: { kind: "catalog" as const, locator: "routine-a" },
	trust: "verified" as const,
	version: "1.0.0",
};

const MakePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-routine-service-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};
let id_sequence = 0;
const MetadataLive = Layer.succeed(RuntimeMetadata, {
	instance_id: "routine_service_test",
	MakeId: (prefix) => Effect.sync(() => `${prefix}_${++id_sequence}`),
	Now: Effect.succeed("2026-07-18T12:00:00.000Z"),
});
const MakeRuntime = (
	database_path: string,
	installs: Array<string>,
	rollbacks: Array<string>,
	fail_install = false,
	drift_actions?: Array<string>,
	inspect: (source: RoutineSource) => RoutineInspection = (source) => ({
		...inspection,
		source,
	}),
	install_invocations?: Array<string>,
	rollback_invocations?: Array<string>,
) => {
	const completed_rollbacks = rollback_operation_ids.get(rollbacks) ?? new Set<string>();
	rollback_operation_ids.set(rollbacks, completed_rollbacks);
	return ManagedRuntime.make(
		RoutineServiceLive.pipe(
			Layer.provideMerge(
				RoutineRepositoryLive.pipe(
					Layer.provideMerge(
						Layer.mergeAll(
							make_database_layer({ database_path, migrations_path }),
							JournalNotifierLive,
							MetadataLive,
						),
					),
				),
			),
			Layer.provideMerge(
				Layer.mergeAll(
					Layer.succeed(RoutineSourceInspector, {
						Inspect: ({ source }) => Effect.succeed(inspect(source)),
					}),
					Layer.succeed(RoutineInstaller, {
						Install: ({ operation_id }) =>
							fail_install
								? Effect.fail(new RoutineInstallerError({ code: "install_failed" }))
								: Effect.sync(() => {
										install_invocations?.push(operation_id);
										if (!installs.includes(operation_id))
											installs.push(operation_id);
										return {
											artifact_refs: inspection.artifact_refs,
											rollback_id: "rollback_a",
										};
									}),
						Rollback: ({ operation_id, rollback_id }) =>
							Effect.sync(() => {
								rollback_invocations?.push(operation_id);
								if (completed_rollbacks.has(operation_id)) return;
								completed_rollbacks.add(operation_id);
								rollbacks.push(rollback_id);
							}),
					}),
					Layer.succeed(RoutineMirrorRegistry, {
						Find: (engine_id) =>
							drift_actions === undefined
								? undefined
								: {
										engine_id,
										mode: "native" as const,
										ResolveDrift: ({ action, routine }) =>
											Effect.sync(() => {
												drift_actions.push(action);
												return action === "import"
													? {
															imported: {
																...routine,
																description:
																	"Imported provider routine",
															},
															revision: "provider_revision",
														}
													: { revision: "provider_revision" };
											}),
										Sync: () =>
											Effect.sync(() => {
												drift_actions.push("sync");
												return { revision: "provider_revision" };
											}),
									},
					}),
					Layer.succeed(NpxSkillsAdapter, {
						Discover: ({ package_spec }) =>
							Effect.succeed({
								candidates: [
									{
										description: "npx candidate",
										files: inspection.files,
										name: "routine-a",
										source_locator: "npx skills routine-a",
										version: "1.0.0",
									},
								],
								package_spec,
							}),
					}),
				),
			),
		),
	);
};

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("RoutineService", () => {
	it("never installs before durable approval and supports progressive browse/detail", async () => {
		const installs: Array<string> = [];
		const rollbacks: Array<string> = [];
		const runtime = MakeRuntime(await MakePath(), installs, rollbacks);
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* RoutineService;
					const preview = yield* service.Preview({
						scope: { kind: "global" },
						source: inspection.source,
					});
					yield* service.RequestInstall({
						approval_id: "approval_a",
						operation_id: "operation_a",
						preview_fingerprint: preview.preview_fingerprint,
						request_fingerprint: "request_a",
						requested_by: "user",
						scope: { kind: "global" },
						source: inspection.source,
					});
					const before = yield* service.Browse({ category: "routine" });
					const detail = yield* service.DecideInstall({
						approved: true,
						approval_id: "approval_a",
						operation_id: "operation_a",
						preview_fingerprint: preview.preview_fingerprint,
						request_fingerprint: "request_a",
						requested_by: "user",
						scope: { kind: "global" },
						source: inspection.source,
					});
					return { before, detail, after: yield* service.Browse({ text: "routine" }) };
				}),
			);
			expect(installs).toEqual(["operation_a"]);
			expect(result.before).toEqual([]);
			expect(result.after).toHaveLength(1);
			expect(result.detail.instructions).toBe(inspection.instructions);
		} finally {
			await runtime.dispose();
		}
	});

	it("records denied approval without calling the installer", async () => {
		const installs: Array<string> = [];
		const runtime = MakeRuntime(await MakePath(), installs, []);
		try {
			const exit = await runtime.runPromiseExit(
				Effect.gen(function* () {
					const service = yield* RoutineService;
					const preview = yield* service.Preview({
						scope: { kind: "global" },
						source: inspection.source,
					});
					yield* service.RequestInstall({
						approval_id: "approval_a",
						operation_id: "operation_a",
						preview_fingerprint: preview.preview_fingerprint,
						request_fingerprint: "request_a",
						requested_by: "user",
						scope: { kind: "global" },
						source: inspection.source,
					});
					yield* service.DecideInstall({
						approved: false,
						approval_id: "approval_a",
						operation_id: "operation_a",
						preview_fingerprint: preview.preview_fingerprint,
						request_fingerprint: "request_a",
						requested_by: "user",
						scope: { kind: "global" },
						source: inspection.source,
					});
				}),
			);
			expect(exit._tag).toBe("Failure");
			expect(installs).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("journals an installer failure after approval without creating a routine", async () => {
		const runtime = MakeRuntime(await MakePath(), [], [], true);
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* RoutineService;
					const preview = yield* service.Preview({
						scope: { kind: "global" },
						source: inspection.source,
					});
					yield* service.RequestInstall({
						approval_id: "approval_failure",
						operation_id: "operation_failure",
						preview_fingerprint: preview.preview_fingerprint,
						request_fingerprint: "request_failure",
						requested_by: "user",
						scope: { kind: "global" },
						source: inspection.source,
					});
					const exit = yield* Effect.exit(
						service.DecideInstall({
							approved: true,
							approval_id: "approval_failure",
							operation_id: "operation_failure",
							preview_fingerprint: preview.preview_fingerprint,
							request_fingerprint: "request_failure",
							requested_by: "user",
							scope: { kind: "global" },
							source: inspection.source,
						}),
					);
					const repository = yield* RoutineRepository;
					return {
						exit,
						recovery: yield* repository.ReadRecovery,
						summaries: yield* repository.ReadSummaries,
					};
				}),
			);
			expect(result.exit._tag).toBe("Failure");
			expect(result.recovery).toEqual([]);
			expect(result.summaries).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("normalizes npx discovery into a preview without making its format canonical", async () => {
		const runtime = MakeRuntime(await MakePath(), [], []);
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* RoutineService;
					const discovery = yield* service.DiscoverNpxSkills({
						package_spec: "skills@1",
						scope: { kind: "global" },
					});
					const candidate = discovery.candidates[0]!;
					const preview = yield* service.PreviewNpxImport({
						candidate_name: candidate.name,
						package_spec: discovery.package_spec,
						preview_fingerprint: candidate.preview_fingerprint!,
						scope: { kind: "global" },
					});
					return { discovery, preview };
				}),
			);
			expect(result.discovery.candidates).toHaveLength(1);
			expect(result.discovery.candidates[0]?.preview_fingerprint).toMatch(/^[a-f0-9]{64}$/);
			expect(result.preview.source).toMatchObject({
				kind: "local",
				locator: "npx skills routine-a",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("executes explicit provider overwrite drift reconciliation and persists synced state", async () => {
		const actions: Array<string> = [];
		const runtime = MakeRuntime(await MakePath(), [], [], false, actions);
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* RoutineRepository;
					yield* repository.RecordPendingInstall({
						approval_fingerprint: "preview_drift",
						approval_id: "approval_drift",
						operation_id: "operation_drift",
						preview_json: "{}",
						request_fingerprint: "request_drift",
						routine_id: "routine_a",
					});
					yield* repository.DecideInstall({
						approval_fingerprint: "preview_drift",
						approval_id: "approval_drift",
						approved: true,
						operation_id: "operation_drift",
					});
					yield* repository.CommitInstalled({
						artifact_refs: [],
						detail: {
							...inspection,
							enabled: true,
							id: "routine_a",
							scope: { kind: "global" },
							status: "enabled",
							sync: [],
						},
						operation_id: "operation_drift",
					});
					const service = yield* RoutineService;
					yield* service.ExecuteApprovedDriftOverwrite({
						engine_id: "codex",
						observed_revision: "external_revision",
						operation_id: "resolve_drift",
						routine_id: "routine_a",
					});
					yield* service.ExecuteApprovedDriftOverwrite({
						engine_id: "codex",
						observed_revision: "external_revision",
						operation_id: "resolve_drift",
						routine_id: "routine_a",
					});
					return yield* service.Detail("routine_a");
				}),
			);
			expect(actions).toEqual(["overwrite"]);
			expect(result.sync).toEqual([
				{
					engine_id: "codex",
					observed_revision: "provider_revision",
					status: "synced",
					updated_at: "2026-07-18T12:00:00.000Z",
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("imports a provider routine only when the adapter returns canonical replacement metadata", async () => {
		const actions: Array<string> = [];
		const runtime = MakeRuntime(await MakePath(), [], [], false, actions);
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* RoutineRepository;
					yield* repository.RecordPendingInstall({
						approval_fingerprint: "preview_import",
						approval_id: "approval_import",
						operation_id: "operation_import",
						preview_json: "{}",
						request_fingerprint: "request_import",
						routine_id: "routine_a",
					});
					yield* repository.DecideInstall({
						approval_fingerprint: "preview_import",
						approval_id: "approval_import",
						approved: true,
						operation_id: "operation_import",
					});
					yield* repository.CommitInstalled({
						artifact_refs: [],
						detail: {
							...inspection,
							enabled: true,
							id: "routine_a",
							scope: { kind: "global" },
							status: "enabled",
							sync: [],
						},
						operation_id: "operation_import",
					});
					const service = yield* RoutineService;
					yield* service.ResolveDrift({
						action: "import",
						engine_id: "codex",
						observed_revision: "external_revision",
						operation_id: "resolve_import",
						routine_id: "routine_a",
					});
					return yield* service.Detail("routine_a");
				}),
			);
			expect(actions).toEqual(["import"]);
			expect(result.description).toBe("Imported provider routine");
			expect(result.sync[0]?.status).toBe("synced");
		} finally {
			await runtime.dispose();
		}
	});

	it("makes ignore an explicit provider reconciliation with a durable ignored state", async () => {
		const actions: Array<string> = [];
		const runtime = MakeRuntime(await MakePath(), [], [], false, actions);
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* RoutineRepository;
					yield* repository.RecordPendingInstall({
						approval_fingerprint: "preview_ignore",
						approval_id: "approval_ignore",
						operation_id: "operation_ignore",
						preview_json: "{}",
						request_fingerprint: "request_ignore",
						routine_id: "routine_a",
					});
					yield* repository.DecideInstall({
						approval_fingerprint: "preview_ignore",
						approval_id: "approval_ignore",
						approved: true,
						operation_id: "operation_ignore",
					});
					yield* repository.CommitInstalled({
						artifact_refs: [],
						detail: {
							...inspection,
							enabled: true,
							id: "routine_a",
							scope: { kind: "global" },
							status: "enabled",
							sync: [],
						},
						operation_id: "operation_ignore",
					});
					const service = yield* RoutineService;
					yield* service.ResolveDrift({
						action: "ignore",
						engine_id: "codex",
						observed_revision: "external_revision",
						operation_id: "resolve_ignore",
						routine_id: "routine_a",
					});
					return yield* service.Detail("routine_a");
				}),
			);
			expect(actions).toEqual(["ignore"]);
			expect(result.sync[0]?.status).toBe("drift_ignored");
		} finally {
			await runtime.dispose();
		}
	});

	it("uses runtime-only sync explicitly and rejects unsupported invocation", async () => {
		const runtime = MakeRuntime(await MakePath(), [], []);
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
					yield* repository.DecideInstall({
						approval_fingerprint: "preview_a",
						approval_id: "approval_a",
						approved: true,
						operation_id: "operation_a",
					});
					yield* repository.CommitInstalled({
						artifact_refs: [],
						detail: {
							...inspection,
							enabled: true,
							id: "routine_a",
							scope: { kind: "global" },
							status: "enabled",
							sync: [],
						},
						operation_id: "operation_a",
					});
					const service = yield* RoutineService;
					yield* service.Sync({
						engine_id: "other",
						operation_id: "sync_a",
						routine_id: "routine_a",
					});
					return yield* service.Detail("routine_a");
				}),
			);
			expect(result.sync).toEqual([
				{
					engine_id: "other",
					status: "runtime_only",
					updated_at: "2026-07-18T12:00:00.000Z",
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("rebuilds the canonical detail from SQLite after a service restart", async () => {
		const database_path = await MakePath();
		const first = MakeRuntime(database_path, [], []);
		try {
			await first.runPromise(
				Effect.gen(function* () {
					const service = yield* RoutineService;
					const preview = yield* service.Preview({
						scope: { kind: "global" },
						source: inspection.source,
					});
					yield* service.Install({
						approved: true,
						approval_id: "approval_restart",
						operation_id: "operation_restart",
						preview_fingerprint: preview.preview_fingerprint,
						request_fingerprint: "request_restart",
						requested_by: "user",
						scope: { kind: "global" },
						source: inspection.source,
					});
				}),
			);
		} finally {
			await first.dispose();
		}
		const second = MakeRuntime(database_path, [], []);
		try {
			const detail = await second.runPromise(
				Effect.gen(function* () {
					return yield* (yield* RoutineService).Detail("routine_a");
				}),
			);
			expect(detail).toMatchObject({
				id: "routine_a",
				instructions: inspection.instructions,
				status: "enabled",
			});
		} finally {
			await second.dispose();
		}
	});

	it("requires re-approval when the source changes during post-approval inspection", async () => {
		let inspections = 0;
		const runtime = MakeRuntime(await MakePath(), [], [], false, undefined, (source) => {
			inspections += 1;
			return {
				...inspection,
				content_hashes: { "SKILL.md": inspections === 3 ? "changed" : "hash_a" },
				source,
			};
		});
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* RoutineService;
					const preview = yield* service.Preview({
						scope: { kind: "global" },
						source: inspection.source,
					});
					yield* service.RequestInstall({
						approval_id: "approval_changed",
						operation_id: "operation_changed",
						preview_fingerprint: preview.preview_fingerprint,
						request_fingerprint: "request_changed",
						requested_by: "user",
						scope: { kind: "global" },
						source: inspection.source,
					});
					const exit = yield* Effect.exit(
						service.DecideInstall({
							approved: true,
							approval_id: "approval_changed",
							operation_id: "operation_changed",
							preview_fingerprint: preview.preview_fingerprint,
							request_fingerprint: "request_changed",
							requested_by: "user",
							scope: { kind: "global" },
							source: inspection.source,
						}),
					);
					return { exit, summaries: yield* service.Browse({}) };
				}),
			);
			expect(result.exit._tag).toBe("Failure");
			expect(result.summaries).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("resumes an approved install after a crash instead of reading missing detail", async () => {
		const installs: Array<string> = [];
		const database_path = await MakePath();
		const first = MakeRuntime(database_path, installs, []);
		try {
			await first.runPromise(
				Effect.gen(function* () {
					const service = yield* RoutineService;
					const preview = yield* service.Preview({
						scope: { kind: "global" },
						source: inspection.source,
					});
					yield* service.RequestInstall({
						approval_id: "approval_resume",
						operation_id: "operation_resume",
						preview_fingerprint: preview.preview_fingerprint,
						request_fingerprint: "request_resume",
						requested_by: "user",
						scope: { kind: "global" },
						source: inspection.source,
					});
					yield* (yield* RoutineRepository).DecideInstall({
						approval_fingerprint: preview.preview_fingerprint,
						approval_id: "approval_resume",
						approved: true,
						operation_id: "operation_resume",
					});
				}),
			);
		} finally {
			await first.dispose();
		}
		const second = MakeRuntime(database_path, installs, []);
		try {
			const detail = await second.runPromise(
				Effect.gen(function* () {
					const service = yield* RoutineService;
					const preview = yield* service.Preview({
						scope: { kind: "global" },
						source: inspection.source,
					});
					return yield* service.DecideInstall({
						approved: true,
						approval_id: "approval_resume",
						operation_id: "operation_resume",
						preview_fingerprint: preview.preview_fingerprint,
						request_fingerprint: "request_resume",
						requested_by: "user",
						scope: { kind: "global" },
						source: inspection.source,
					});
				}),
			);
			expect(detail.id).toBe("routine_a");
			expect(installs).toEqual(["operation_resume"]);
		} finally {
			await second.dispose();
		}
	});

	it("enforces workspace scope before journaling routine invocation", async () => {
		const runtime = MakeRuntime(await MakePath(), [], []);
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* RoutineRepository;
					yield* repository.RecordPendingInstall({
						approval_fingerprint: "preview_scope",
						approval_id: "approval_scope",
						operation_id: "operation_scope",
						preview_json: "{}",
						request_fingerprint: "request_scope",
						routine_id: "routine_a",
					});
					yield* repository.DecideInstall({
						approval_fingerprint: "preview_scope",
						approval_id: "approval_scope",
						approved: true,
						operation_id: "operation_scope",
					});
					yield* repository.CommitInstalled({
						artifact_refs: [],
						detail: {
							...inspection,
							enabled: true,
							id: "routine_a",
							scope: { kind: "workspace", workspace_id: "workspace_a" },
							status: "enabled",
							sync: [],
						},
						operation_id: "operation_scope",
					});
					const service = yield* RoutineService;
					const denied = yield* Effect.exit(
						service.Invoke({
							engine_id: "codex",
							operation_id: "invoke_denied",
							routine_id: "routine_a",
							scope: { kind: "workspace", workspace_id: "workspace_b" },
							task_summary: "Denied task",
						}),
					);
					const allowed = yield* service.Invoke({
						engine_id: "codex",
						operation_id: "invoke_allowed",
						routine_id: "routine_a",
						scope: { kind: "workspace", workspace_id: "workspace_a" },
						task_summary: "Allowed task",
					});
					const exported = yield* service.Invoke({
						command: "routine-a",
						engine_id: "codex",
						operation_id: "invoke_exported",
						routine_id: "routine_a",
						scope: { kind: "workspace", workspace_id: "workspace_a" },
						task_summary: "Exported command",
					});
					const unexported = yield* Effect.exit(
						service.Invoke({
							command: "not-exported",
							engine_id: "codex",
							operation_id: "invoke_unexported",
							routine_id: "routine_a",
							scope: { kind: "workspace", workspace_id: "workspace_a" },
							task_summary: "Invalid command",
						}),
					);
					return { allowed, denied, exported, unexported };
				}),
			);
			expect(result.denied._tag).toBe("Failure");
			expect(result.allowed.eligible).toBe(true);
			expect(result.exported.eligible).toBe(true);
			expect(result.unexported._tag).toBe("Failure");
		} finally {
			await runtime.dispose();
		}
	});

	it("recovers a claimed rollback once and makes the exact retry side-effect free", async () => {
		const database_path = await MakePath();
		const rollbacks: Array<string> = [];
		const rollback_invocations: Array<string> = [];
		const provider_actions: Array<string> = [];
		const first = MakeRuntime(
			database_path,
			[],
			rollbacks,
			false,
			provider_actions,
			undefined,
			undefined,
			rollback_invocations,
		);
		try {
			await first.runPromise(
				Effect.gen(function* () {
					const service = yield* RoutineService;
					const preview = yield* service.Preview({
						scope: { kind: "global" },
						source: inspection.source,
					});
					yield* service.Install({
						approved: true,
						approval_id: "approval_rollback",
						operation_id: "operation_install_rollback",
						preview_fingerprint: preview.preview_fingerprint,
						request_fingerprint: "request_rollback",
						requested_by: "user",
						scope: { kind: "global" },
						source: inspection.source,
					});
					yield* (yield* RoutineRepository).ClaimRollback({
						operation_id: "operation_rollback",
						rollback_id: "rollback_a",
						routine_id: "routine_a",
					});
					/** Simulates a process crash after the external rollback effect but before commit. */
					yield* (yield* RoutineInstaller).Rollback({
						operation_id: "operation_rollback",
						rollback_id: "rollback_a",
					});
				}),
			);
		} finally {
			await first.dispose();
		}
		const second = MakeRuntime(
			database_path,
			[],
			rollbacks,
			false,
			provider_actions,
			undefined,
			undefined,
			rollback_invocations,
		);
		try {
			const result = await second.runPromise(
				Effect.gen(function* () {
					const service = yield* RoutineService;
					yield* service.RecoverRollbacks;
					yield* service.Rollback({
						operation_id: "operation_rollback",
						rollback_id: "rollback_a",
						routine_id: "routine_a",
					});
					const fresh_operation = yield* Effect.exit(
						service.Rollback({
							operation_id: "operation_rollback_fresh",
							rollback_id: "rollback_a",
							routine_id: "routine_a",
						}),
					);
					const detail = yield* service.Detail("routine_a");
					const enable = yield* Effect.exit(
						service.Enable({
							operation_id: "enable_rolled_back",
							routine_id: "routine_a",
						}),
					);
					const terminal_operations = yield* Effect.all([
						Effect.exit(
							service.Disable({
								operation_id: "disable_rolled_back",
								routine_id: "routine_a",
							}),
						),
						Effect.exit(
							service.Remove({
								operation_id: "remove_rolled_back",
								routine_id: "routine_a",
							}),
						),
						Effect.exit(
							service.Sync({
								engine_id: "codex",
								operation_id: "sync_rolled_back",
								routine_id: "routine_a",
							}),
						),
						Effect.exit(
							service.ExecuteApprovedDriftOverwrite({
								engine_id: "codex",
								observed_revision: "external",
								operation_id: "drift_rolled_back",
								routine_id: "routine_a",
							}),
						),
					]);
					return { detail, enable, fresh_operation, terminal_operations };
				}),
			);
			expect(rollbacks).toEqual(["rollback_a"]);
			expect(rollback_invocations).toEqual(["operation_rollback", "operation_rollback"]);
			expect(result.detail.status).toBe("rolled_back");
			expect(result.enable._tag).toBe("Failure");
			expect(result.fresh_operation._tag).toBe("Failure");
			expect(
				result.terminal_operations.every((operation) => operation._tag === "Failure"),
			).toBe(true);
			expect(provider_actions).toEqual([]);
		} finally {
			await second.dispose();
		}
	});

	it("rejects every mutable operation after removal without provider or rollback effects", async () => {
		const provider_actions: Array<string> = [];
		const rollback_invocations: Array<string> = [];
		const runtime = MakeRuntime(
			await MakePath(),
			[],
			[],
			false,
			provider_actions,
			undefined,
			undefined,
			rollback_invocations,
		);
		try {
			const exits = await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* RoutineService;
					const preview = yield* service.Preview({
						scope: { kind: "global" },
						source: inspection.source,
					});
					yield* service.Install({
						approved: true,
						approval_id: "approval_terminal",
						operation_id: "install_terminal",
						preview_fingerprint: preview.preview_fingerprint,
						request_fingerprint: "request_terminal",
						requested_by: "user",
						scope: { kind: "global" },
						source: inspection.source,
					});
					yield* service.Remove({
						operation_id: "remove_terminal",
						routine_id: "routine_a",
					});
					return yield* Effect.all({
						disable: Effect.exit(
							service.Disable({
								operation_id: "disable_terminal",
								routine_id: "routine_a",
							}),
						),
						drift: Effect.exit(
							service.ExecuteApprovedDriftOverwrite({
								engine_id: "codex",
								observed_revision: "external",
								operation_id: "drift_terminal",
								routine_id: "routine_a",
							}),
						),
						enable: Effect.exit(
							service.Enable({
								operation_id: "enable_terminal",
								routine_id: "routine_a",
							}),
						),
						remove: Effect.exit(
							service.Remove({
								operation_id: "remove_terminal_again",
								routine_id: "routine_a",
							}),
						),
						rollback: Effect.exit(
							service.Rollback({
								operation_id: "rollback_terminal",
								rollback_id: "rollback_a",
								routine_id: "routine_a",
							}),
						),
						sync: Effect.exit(
							service.Sync({
								engine_id: "codex",
								operation_id: "sync_terminal",
								routine_id: "routine_a",
							}),
						),
					});
				}),
			);
			expect(Object.values(exits).every((exit) => exit._tag === "Failure")).toBe(true);
			expect(provider_actions).toEqual([]);
			expect(rollback_invocations).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("binds rollback authority to the persisted successful install receipt", async () => {
		const rollbacks: Array<string> = [];
		const runtime = MakeRuntime(await MakePath(), [], rollbacks);
		try {
			const exit = await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* RoutineService;
					const preview = yield* service.Preview({
						scope: { kind: "global" },
						source: inspection.source,
					});
					yield* service.Install({
						approved: true,
						approval_id: "approval_bound",
						operation_id: "install_bound",
						preview_fingerprint: preview.preview_fingerprint,
						request_fingerprint: "request_bound",
						requested_by: "user",
						scope: { kind: "global" },
						source: inspection.source,
					});
					return yield* Effect.exit(
						service.Rollback({
							operation_id: "rollback_forged",
							rollback_id: "forged",
							routine_id: "routine_a",
						}),
					);
				}),
			);
			expect(exit._tag).toBe("Failure");
			expect(rollbacks).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("does not duplicate an install effect when recovery follows a crash after the effect", async () => {
		const database_path = await MakePath();
		const installs: Array<string> = [];
		const install_invocations: Array<string> = [];
		const first = MakeRuntime(
			database_path,
			installs,
			[],
			false,
			undefined,
			undefined,
			install_invocations,
		);
		let preview_fingerprint = "";
		try {
			await first.runPromise(
				Effect.gen(function* () {
					const service = yield* RoutineService;
					const preview = yield* service.Preview({
						scope: { kind: "global" },
						source: inspection.source,
					});
					preview_fingerprint = preview.preview_fingerprint;
					yield* service.RequestInstall({
						approval_id: "approval_after_effect",
						operation_id: "install_after_effect",
						preview_fingerprint,
						request_fingerprint: "request_after_effect",
						requested_by: "user",
						scope: { kind: "global" },
						source: inspection.source,
					});
					yield* (yield* RoutineRepository).DecideInstall({
						approval_fingerprint: preview_fingerprint,
						approval_id: "approval_after_effect",
						approved: true,
						operation_id: "install_after_effect",
					});
					yield* (yield* RoutineInstaller).Install({
						inspection,
						operation_id: "install_after_effect",
						scope: { kind: "global" },
					});
				}),
			);
		} finally {
			await first.dispose();
		}
		const second = MakeRuntime(
			database_path,
			installs,
			[],
			false,
			undefined,
			undefined,
			install_invocations,
		);
		try {
			await second.runPromise(
				Effect.gen(function* () {
					return yield* (yield* RoutineService).DecideInstall({
						approved: true,
						approval_id: "approval_after_effect",
						operation_id: "install_after_effect",
						preview_fingerprint,
						request_fingerprint: "request_after_effect",
						requested_by: "user",
						scope: { kind: "global" },
						source: inspection.source,
					});
				}),
			);
			expect(installs).toEqual(["install_after_effect"]);
			expect(install_invocations).toEqual(["install_after_effect", "install_after_effect"]);
		} finally {
			await second.dispose();
		}
	});

	it("rejects malformed inspector output at the adapter boundary", async () => {
		const runtime = MakeRuntime(await MakePath(), [], [], false, undefined, (source) => ({
			...inspection,
			author: "",
			source,
		}));
		try {
			const exit = await runtime.runPromiseExit(
				Effect.gen(function* () {
					yield* (yield* RoutineService).Preview({
						scope: { kind: "global" },
						source: inspection.source,
					});
				}),
			);
			expect(exit._tag).toBe("Failure");
		} finally {
			await runtime.dispose();
		}
	});
});
