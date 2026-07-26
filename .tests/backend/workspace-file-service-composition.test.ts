import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Deferred, Effect, Exit, Layer, Semaphore } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { ContentIdentity } from "@artisan/protocol";
import {
	make_backend_runtime,
	type BoundedRegularFileStore,
	BoundedRegularFileStoreError,
	type ReplaceRegularFileOptions,
	ProtocolRouter,
	ThreadRetentionClock,
	ThreadRetentionScheduler,
	WorkspaceChangeRepository,
	WorkspaceFileService,
	WorkspaceMutationPayloadStore,
	WorkspaceSnapshotStore,
} from "@artisan/backend";

import { WorkspaceBoundedRegularFileStoreRegistry } from "../../modules/backend/src/filesystem/workspace-bounded-regular-file-store-registry";
import { MakeTestWorkspaceBoundedRegularFileStoreRegistryLayer } from "./bounded-regular-file-store-harness";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import {
	OrchestrationCoordinators,
	OrchestrationRuns,
	ThreadErasureClaims,
	WorkspaceChangeOperations,
	WorkspaceChangeSnapshots,
	WorkspaceChanges,
	WorkspaceMutationAuthorities,
	WorkspaceMutationPayloads,
} from "../../modules/backend/src/persistence/schema";
import { Database } from "../../modules/backend/src/persistence/database";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const now = "2026-07-12T13:00:00.000Z";
const encoder = new TextEncoder();
const decoder = new TextDecoder();

type BoundedStoreController = {
	readonly arm_preflight_race: (gate: PreflightRaceGate) => void;
	readonly change_next_replacement: () => void;
	readonly changed_results: { value: number };
	readonly finalization_attempts: { value: number };
	readonly load_attempts: { value: number };
	readonly replace_attempts: { value: number };
	readonly write_attempts: { value: number };
	readonly fail_next_finalization: () => void;
	readonly MakeStore: (root: string) => typeof BoundedRegularFileStore.Service;
};

type ReplacementGate = {
	readonly continue_first: Deferred.Deferred<void>;
	readonly continue_second: Deferred.Deferred<void>;
	readonly first_started: Deferred.Deferred<void>;
	readonly second_started: Deferred.Deferred<void>;
};

type PreflightRaceGate = {
	readonly continue_first_read: Deferred.Deferred<void>;
	readonly continue_publisher: Deferred.Deferred<void>;
	readonly continue_second_read: Deferred.Deferred<void>;
	readonly first_read_started: Deferred.Deferred<void>;
	readonly published: Deferred.Deferred<void>;
	readonly second_read_started: Deferred.Deferred<void>;
};

type AuthorityProofGate = {
	readonly continue_proof: Deferred.Deferred<void>;
	readonly proof_started: Deferred.Deferred<void>;
};

function content_identity(bytes: Uint8Array): ContentIdentity {
	return {
		algorithm: "sha256",
		byte_count: bytes.byteLength,
		content_hash: createHash("sha256").update(bytes).digest("hex"),
	};
}

function payload_input(content: string) {
	const expected = encoder.encode("before");
	const replacement = encoder.encode(content);

	return {
		action: "replace" as const,
		expected_identity: content_identity(expected),
		message_id: "replace_message",
		replacement_identity: content_identity(replacement),
		thread_id: "thread_workspace_file",
	};
}

function rollback_payload_input() {
	return {
		action: "rollback" as const,
		expected_identity: content_identity(encoder.encode("after")),
		message_id: "rollback_message",
		replacement_identity: content_identity(encoder.encode("before")),
		thread_id: "thread_workspace_file",
	};
}

function replacement_input(content = "after") {
	return {
		agent_id: "agent_workspace_file",
		change_id: "change_workspace_file",
		content,
		expected_before: content_identity(encoder.encode("before")),
		message_id: "replace_message",
		path: "src/example.ts",
		run_id: "run_workspace_file",
		sent_at: now,
		thread_id: "thread_workspace_file",
		workspace_id: "workspace_file",
	};
}

function review_input() {
	return {
		change_id: "change_workspace_file",
		message_id: "review_message",
		sent_at: now,
		thread_id: "thread_workspace_file",
	};
}

function rollback_input() {
	return {
		change_id: "change_workspace_file",
		expected_after: content_identity(encoder.encode("after")),
		message_id: "rollback_message",
		sent_at: now,
		thread_id: "thread_workspace_file",
	};
}

function create_thread_command() {
	return {
		kind: "command" as const,
		message_id: "create_workspace_file_thread",
		origin: "frontend" as const,
		payload: {
			title: "Workspace file composition",
			type: "thread.create" as const,
		},
		protocol_version: 1 as const,
		schema_version: 1 as const,
		sent_at: now,
		thread_id: "thread_workspace_file",
	};
}

function bytes_match(left: Uint8Array, right: Uint8Array) {
	return (
		left.byteLength === right.byteLength && left.every((value, index) => value === right[index])
	);
}

function replacement_options_match(
	left: ReplaceRegularFileOptions,
	right: ReplaceRegularFileOptions,
) {
	return (
		left.maximum_bytes === right.maximum_bytes &&
		left.operation_id === right.operation_id &&
		left.path === right.path &&
		bytes_match(left.expected, right.expected) &&
		bytes_match(left.replacement, right.replacement)
	);
}

function make_bounded_store_controller(replacement_gate?: ReplacementGate): BoundedStoreController {
	const changed_results = { value: 0 };
	const finalization_attempts = { value: 0 };
	const load_attempts = { value: 0 };
	const replace_attempts = { value: 0 };
	const write_attempts = { value: 0 };
	const receipts = new Map<string, ReplaceRegularFileOptions>();
	const replacement_lock = Semaphore.makeUnsafe(1);
	let preflight_race_gate: PreflightRaceGate | undefined;
	let preflight_read_attempts = 0;
	let remaining_changed_results = 0;
	let remaining_finalization_failures = 0;

	const MakeStore = (root: string): typeof BoundedRegularFileStore.Service => ({
		FinalizeRegularFileReplacement: (options) =>
			Effect.gen(function* () {
				finalization_attempts.value += 1;

				if (remaining_finalization_failures > 0) {
					remaining_finalization_failures -= 1;

					return yield* new BoundedRegularFileStoreError({
						cause: new Error("deterministic finalization failure"),
						operation: "finalize",
						path: options.path,
					});
				}

				const receipt = receipts.get(options.operation_id);

				if (receipt === undefined) {
					const target = yield* Effect.tryPromise(() =>
						readFile(join(root, options.path)),
					);

					if (!bytes_match(target, options.replacement)) {
						return yield* new BoundedRegularFileStoreError({
							cause: new Error("replacement receipt is missing before publication"),
							operation: "finalize",
							path: options.path,
						});
					}

					return;
				}

				if (!replacement_options_match(receipt, options)) {
					return yield* new BoundedRegularFileStoreError({
						cause: new Error("replacement receipt intent changed"),
						operation: "finalize",
						path: options.path,
					});
				}

				receipts.delete(options.operation_id);
			}).pipe(
				Effect.mapError((cause) =>
					cause instanceof BoundedRegularFileStoreError
						? cause
						: new BoundedRegularFileStoreError({
								cause,
								operation: "finalize",
								path: options.path,
							}),
				),
			),
		ReadRegularFile: (path, maximum_bytes) =>
			Effect.gen(function* () {
				if (preflight_race_gate) {
					preflight_read_attempts += 1;
					const attempt = preflight_read_attempts;

					if (attempt === 1) {
						yield* Deferred.succeed(preflight_race_gate.first_read_started, undefined);
						yield* Deferred.await(preflight_race_gate.continue_first_read);
					}

					if (attempt === 2) {
						yield* Deferred.succeed(preflight_race_gate.second_read_started, undefined);
						yield* Deferred.await(preflight_race_gate.continue_second_read);
					}
				}

				const bytes = yield* Effect.tryPromise(() => readFile(join(root, path)));

				if (bytes.byteLength > maximum_bytes) {
					return yield* new BoundedRegularFileStoreError({
						cause: new Error("file exceeds maximum"),
						operation: "read",
						path,
					});
				}

				return bytes;
			}).pipe(
				Effect.mapError((cause) =>
					cause instanceof BoundedRegularFileStoreError
						? cause
						: new BoundedRegularFileStoreError({ cause, operation: "read", path }),
				),
			),
		ReplaceRegularFile: (options) =>
			Effect.gen(function* () {
				replace_attempts.value += 1;
				const attempt = replace_attempts.value;

				if (replacement_gate && attempt === 1) {
					yield* Deferred.succeed(replacement_gate.first_started, undefined);
					yield* Deferred.await(replacement_gate.continue_first);
				}

				if (replacement_gate && attempt === 2) {
					yield* Deferred.succeed(replacement_gate.second_started, undefined);
					yield* Deferred.await(replacement_gate.continue_second);
				}

				const result = yield* replacement_lock.withPermit(
					Effect.gen(function* () {
						const receipt = receipts.get(options.operation_id);

						if (receipt !== undefined) {
							if (!replacement_options_match(receipt, options)) {
								return yield* new BoundedRegularFileStoreError({
									cause: new Error("replacement operation intent changed"),
									operation: "replace",
									path: options.path,
								});
							}

							return { _tag: "AlreadyReplaced" } as const;
						}

						if (remaining_changed_results > 0) {
							remaining_changed_results -= 1;

							return { _tag: "Changed" } as const;
						}

						const target = join(root, options.path);
						const current = yield* Effect.tryPromise(() => readFile(target));
						const matches =
							current.byteLength === options.expected.byteLength &&
							current.every((value, index) => value === options.expected[index]);

						if (!matches || options.replacement.byteLength > options.maximum_bytes) {
							return { _tag: "Changed" } as const;
						}

						write_attempts.value += 1;
						yield* Effect.tryPromise(() => writeFile(target, options.replacement));
						receipts.set(options.operation_id, {
							...options,
							expected: new Uint8Array(options.expected),
							replacement: new Uint8Array(options.replacement),
						});

						return { _tag: "Replaced" } as const;
					}),
				);
				if (result._tag === "Changed") {
					changed_results.value += 1;
				}

				if (result._tag === "Replaced" && preflight_race_gate) {
					yield* Deferred.succeed(preflight_race_gate.published, undefined);
					yield* Deferred.await(preflight_race_gate.continue_publisher);
				}

				return result;
			}).pipe(
				Effect.mapError(
					(cause) =>
						new BoundedRegularFileStoreError({
							cause,
							operation: "replace",
							path: options.path,
						}),
				),
			),
	});

	return {
		arm_preflight_race: (gate) => {
			preflight_race_gate = gate;
			preflight_read_attempts = 0;
		},
		change_next_replacement: () => {
			remaining_changed_results += 1;
		},
		changed_results,
		finalization_attempts,
		load_attempts,
		replace_attempts,
		write_attempts,
		fail_next_finalization: () => {
			remaining_finalization_failures += 1;
		},
		MakeStore: (root) => {
			load_attempts.value += 1;
			return MakeStore(root);
		},
	};
}

async function make_workspace() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-workspace-file-service-composition-"));
	const root = join(directory, "workspace");

	temporary_directories.push(directory);

	await mkdir(join(root, "src"), { recursive: true });
	await writeFile(join(root, "src", "example.ts"), "before");

	return { database_path: join(directory, "artisan.db"), root };
}

function make_metadata_layer(instance_id: string) {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) => Effect.sync(() => `${instance_id}_${prefix}_${++next_id}`),
		Now: Effect.succeed(now),
	});
}

function make_inert_scheduler_layer() {
	return Layer.succeed(ThreadRetentionScheduler, {
		Schedule: () => Effect.never,
	});
}

function make_runtime(
	database_path: string,
	root: string,
	controller?: BoundedStoreController,
	instance_id = "workspace_file_service_composition",
	authority_proof_gate?: AuthorityProofGate,
) {
	const registry =
		controller === undefined
			? undefined
			: MakeTestWorkspaceBoundedRegularFileStoreRegistryLayer([
					{
						root,
						store: controller.MakeStore(root),
						workspace_id: "workspace_file",
					},
				]);
	const workspace_bounded_regular_file_store_registry =
		registry === undefined || authority_proof_gate === undefined
			? registry
			: Layer.effect(
					WorkspaceBoundedRegularFileStoreRegistry,
					Effect.gen(function* () {
						const live = yield* WorkspaceBoundedRegularFileStoreRegistry;
						let paused = false;

						return {
							...live,
							Authorize: (input: Parameters<typeof live.Authorize>[0]) => {
								if (paused) {
									return live.Authorize(input);
								}

								paused = true;

								return Deferred.succeed(
									authority_proof_gate.proof_started,
									undefined,
								).pipe(
									Effect.andThen(
										Deferred.await(authority_proof_gate.continue_proof),
									),
									Effect.andThen(live.Authorize(input)),
								);
							},
						};
					}),
				).pipe(Layer.provide(registry));

	return make_backend_runtime({
		database_path,
		migrations_path,
		retention_clock: Layer.succeed(ThreadRetentionClock, { Now: Effect.succeed(now) }),
		retention_scheduler: make_inert_scheduler_layer(),
		runtime_metadata: make_metadata_layer(instance_id),
		...(workspace_bounded_regular_file_store_registry === undefined
			? {}
			: {
					workspace_bounded_regular_file_store_registry,
				}),
	});
}

function SeedBaseRun(root: string) {
	return Effect.gen(function* () {
		const database = yield* Database;
		const router = yield* ProtocolRouter;

		yield* router.Route(create_thread_command());
		yield* database.client.insert(OrchestrationCoordinators).values({
			active_run_id: "run_workspace_file",
			agent_id: "agent_workspace_file",
			created_at: now,
			display_name: "Coordinator",
			engine_id: "engine_workspace_file",
			role: "primary",
			thread_id: "thread_workspace_file",
			updated_at: now,
		});
		yield* database.client.insert(OrchestrationRuns).values({
			agent_id: "agent_workspace_file",
			created_at: now,
			engine_id: "engine_workspace_file",
			run_id: "run_workspace_file",
			status: "running",
			thread_id: "thread_workspace_file",
			updated_at: now,
			working_directory: root,
		});
	});
}

function TerminalizeBaseRun() {
	return Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client
			.update(OrchestrationRuns)
			.set({ status: "complete", updated_at: now });
	});
}

function InstallSnapshotConsumeFailure() {
	return Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.run(`
			CREATE TRIGGER fail_workspace_snapshot_consume
			BEFORE UPDATE OF state ON workspace_change_snapshots
			WHEN NEW.state = 'consumed'
			BEGIN
				SELECT RAISE(ABORT, 'deterministic snapshot consume failure');
			END
		`);
	});
}

function RemoveSnapshotConsumeFailure() {
	return Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.run("DROP TRIGGER fail_workspace_snapshot_consume");
	});
}

function InstallEvidenceFailure() {
	return Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.run(`
			CREATE TRIGGER fail_workspace_evidence
			BEFORE INSERT ON journal_events
			WHEN NEW.correlation_id = 'workspace_evidence:replace_message:correlation'
			BEGIN
				SELECT RAISE(ABORT, 'deterministic evidence failure');
			END
		`);
	});
}

function RemoveEvidenceFailure() {
	return Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.run("DROP TRIGGER IF EXISTS fail_workspace_evidence");
	});
}

function Replace(input = replacement_input()) {
	return Effect.gen(function* () {
		const service = yield* WorkspaceFileService;

		return yield* service.Replace(input);
	});
}

function ReplaceAfterBarrier(ready: Deferred.Deferred<void>, start: Deferred.Deferred<void>) {
	return Deferred.succeed(ready, undefined).pipe(
		Effect.andThen(Deferred.await(start)),
		Effect.andThen(Replace()),
	);
}

function Review() {
	return Effect.gen(function* () {
		const service = yield* WorkspaceFileService;

		return yield* service.Review(review_input());
	});
}

function Rollback() {
	return Effect.gen(function* () {
		const service = yield* WorkspaceFileService;

		return yield* service.Rollback(rollback_input());
	});
}

function Read() {
	return Effect.gen(function* () {
		const service = yield* WorkspaceFileService;

		return yield* service.Read({ path: "src/example.ts", workspace_id: "workspace_file" });
	});
}

function InspectCommittedReplacement(content: string) {
	const input = replacement_input(content);
	const payload = payload_input(content);

	return Effect.gen(function* () {
		const changes = yield* WorkspaceChangeRepository;
		const database = yield* Database;
		const journal = yield* JournalStore;
		const payloads = yield* WorkspaceMutationPayloadStore;
		const snapshots = yield* WorkspaceSnapshotStore;

		return {
			change_list: yield* changes.List(input.thread_id, input.workspace_id),
			evidence: yield* journal.ReadCorrelatedEvents(
				"workspace_evidence:replace_message:correlation",
			),
			operation: yield* changes.ReadOperation(input.message_id),
			payload_available: yield* payloads.Resume(payload).pipe(Effect.exit),
			payload_record_exists: yield* payloads.HasRecord(payload),
			payload_rows: yield* database.client.select().from(WorkspaceMutationPayloads),
			snapshot: yield* snapshots.Read({
				change_id: input.change_id,
				expected_identity: input.expected_before,
				thread_id: input.thread_id,
			}),
		};
	});
}

function InspectReplacementRows() {
	return Effect.gen(function* () {
		const database = yield* Database;
		const journal = yield* JournalStore;

		return {
			authorities: yield* database.client.select().from(WorkspaceMutationAuthorities),
			changes: yield* database.client.select().from(WorkspaceChanges),
			evidence: yield* journal.ReadCorrelatedEvents(
				"workspace_evidence:replace_message:correlation",
			),
			operations: yield* database.client.select().from(WorkspaceChangeOperations),
			payloads: yield* database.client.select().from(WorkspaceMutationPayloads),
			snapshots: yield* database.client.select().from(WorkspaceChangeSnapshots),
		};
	});
}

function ClaimThreadErasure() {
	return Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(ThreadErasureClaims).values({
			claimed_at: now,
			thread_id: "thread_workspace_file",
		});
	});
}

function InspectRollback() {
	const replacement = replacement_input();
	const rollback = rollback_input();
	const payload = rollback_payload_input();

	return Effect.gen(function* () {
		const changes = yield* WorkspaceChangeRepository;
		const database = yield* Database;
		const journal = yield* JournalStore;
		const payloads = yield* WorkspaceMutationPayloadStore;
		const snapshots = yield* WorkspaceSnapshotStore;

		return {
			change_list: yield* changes.List(replacement.thread_id, replacement.workspace_id),
			evidence: yield* journal.ReadCorrelatedEvents(
				"workspace_evidence:rollback_message:correlation",
			),
			operation: yield* changes.ReadOperation(rollback.message_id),
			payload_available: yield* payloads.Resume(payload).pipe(Effect.exit),
			payload_rows: yield* database.client.select().from(WorkspaceMutationPayloads),
			payload_record_state: yield* payloads.HasRecord(payload).pipe(Effect.exit),
			snapshot_rows: yield* database.client.select().from(WorkspaceChangeSnapshots),
			snapshot_available: yield* snapshots.Exists({
				change_id: rollback.change_id,
				thread_id: rollback.thread_id,
			}),
			snapshot_state: yield* snapshots
				.Read({
					change_id: rollback.change_id,
					expected_identity: replacement.expected_before,
					thread_id: rollback.thread_id,
				})
				.pipe(Effect.exit),
		};
	});
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("WorkspaceFileService production composition", () => {
	it("reads and replaces through SQLite-backed services while retaining the private preimage", async () => {
		const { database_path, root } = await make_workspace();
		const controller = make_bounded_store_controller();
		const runtime = make_runtime(database_path, root, controller);

		try {
			await runtime.runPromise(SeedBaseRun(root));

			const before = await runtime.runPromise(Read());
			const result = await runtime.runPromise(Replace());
			const after = await runtime.runPromise(Read());
			const persisted = await runtime.runPromise(InspectCommittedReplacement("after"));

			expect(before).toMatchObject({
				content: "before",
				identity: content_identity(encoder.encode("before")),
			});
			expect(result.status).toBe("accepted");
			expect(after).toMatchObject({
				content: "after",
				identity: content_identity(encoder.encode("after")),
			});
			expect(decoder.decode(await readFile(join(root, "src", "example.ts")))).toBe("after");
			expect(persisted.operation).toMatchObject({
				_tag: "Some",
				value: { lifecycle: "committed" },
			});
			expect(persisted.change_list.changes).toHaveLength(1);
			expect(persisted.evidence).toMatchObject([
				{ payload: { type: "filesystem.mutation" } },
			]);
			expect(decoder.decode(persisted.snapshot)).toBe("before");
			expect(persisted.payload_record_exists).toBe(true);
			expect(persisted.payload_available).toMatchObject({ _tag: "Failure" });
			expect(controller.replace_attempts.value).toBe(1);
			expect(controller.finalization_attempts.value).toBe(1);
			expect(controller.load_attempts.value).toBe(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("converges concurrent exact replacements across runtimes without publishing twice", async () => {
		const { database_path, root } = await make_workspace();
		const replacement_gate: ReplacementGate = {
			continue_first: await Effect.runPromise(Deferred.make<void>()),
			continue_second: await Effect.runPromise(Deferred.make<void>()),
			first_started: await Effect.runPromise(Deferred.make<void>()),
			second_started: await Effect.runPromise(Deferred.make<void>()),
		};
		const controller = make_bounded_store_controller(replacement_gate);
		const first_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_concurrent_first",
		);
		const second_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_concurrent_second",
		);
		try {
			await first_runtime.runPromise(Read());
			await second_runtime.runPromise(Read());
			await first_runtime.runPromise(SeedBaseRun(root));

			const first_pending = first_runtime.runPromise(Effect.exit(Replace()));

			await Effect.runPromise(Deferred.await(replacement_gate.first_started));

			const second_pending = second_runtime.runPromise(Effect.exit(Replace()));

			await Effect.runPromise(Deferred.await(replacement_gate.second_started));
			await Effect.runPromise(Deferred.succeed(replacement_gate.continue_first, undefined));

			const first_result = await first_pending;

			await Effect.runPromise(Deferred.succeed(replacement_gate.continue_second, undefined));

			const results = [first_result, await second_pending];
			const persisted = await first_runtime.runPromise(InspectCommittedReplacement("after"));
			const successes = results.filter(Exit.isSuccess);

			expect(successes).toHaveLength(2);
			expect(successes.map((result) => result.value.status).toSorted()).toEqual([
				"accepted",
				"duplicate",
			]);
			expect(successes[0]!.value.event).toEqual(successes[1]!.value.event);
			expect(successes[0]!.value.event.payload).toMatchObject({
				type: "workspace.change.updated",
			});
			expect(persisted.change_list.changes).toHaveLength(1);
			expect(persisted.evidence).toMatchObject([
				{ payload: { type: "filesystem.mutation" } },
			]);
			expect(decoder.decode(persisted.snapshot)).toBe("before");
			expect(persisted.payload_record_exists).toBe(true);
			expect(persisted.payload_available).toMatchObject({ _tag: "Failure" });
			expect(persisted.payload_rows).toContainEqual(
				expect.objectContaining({
					expected: null,
					replacement: null,
					state: "consumed",
				}),
			);
			expect(decoder.decode(await readFile(join(root, "src", "example.ts")))).toBe("after");
			expect(controller.changed_results.value).toBe(1);
			expect(controller.replace_attempts.value).toBe(2);
			expect(controller.write_attempts.value).toBe(1);
			expect(controller.load_attempts.value).toBe(2);
		} finally {
			await Effect.runPromise(Deferred.succeed(replacement_gate.continue_first, undefined));
			await Effect.runPromise(Deferred.succeed(replacement_gate.continue_second, undefined));
			await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
		}
	});

	it("preserves an exact replacement published during a retry preflight", async () => {
		const { database_path, root } = await make_workspace();
		const preflight_gate: PreflightRaceGate = {
			continue_first_read: await Effect.runPromise(Deferred.make<void>()),
			continue_publisher: await Effect.runPromise(Deferred.make<void>()),
			continue_second_read: await Effect.runPromise(Deferred.make<void>()),
			first_read_started: await Effect.runPromise(Deferred.make<void>()),
			published: await Effect.runPromise(Deferred.make<void>()),
			second_read_started: await Effect.runPromise(Deferred.make<void>()),
		};
		const controller = make_bounded_store_controller();
		const first_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_preflight_first",
		);
		const second_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_preflight_second",
		);

		try {
			await first_runtime.runPromise(Read());
			await second_runtime.runPromise(Read());
			await first_runtime.runPromise(SeedBaseRun(root));
			controller.arm_preflight_race(preflight_gate);

			const first_pending = first_runtime.runPromise(Effect.exit(Replace()));

			await Effect.runPromise(Deferred.await(preflight_gate.first_read_started));

			const second_pending = second_runtime.runPromise(Effect.exit(Replace()));

			await Effect.runPromise(Deferred.await(preflight_gate.second_read_started));
			await Effect.runPromise(
				Deferred.succeed(preflight_gate.continue_first_read, undefined),
			);
			await Effect.runPromise(Deferred.await(preflight_gate.published));
			await Effect.runPromise(
				Deferred.succeed(preflight_gate.continue_second_read, undefined),
			);

			const second_result = await second_pending;

			await Effect.runPromise(Deferred.succeed(preflight_gate.continue_publisher, undefined));

			const results = [await first_pending, second_result];
			const successes = results.filter(Exit.isSuccess);
			const persisted = await first_runtime.runPromise(InspectCommittedReplacement("after"));

			expect(successes).toHaveLength(2);
			expect(successes.map((result) => result.value.status).toSorted()).toEqual([
				"accepted",
				"duplicate",
			]);
			expect(successes[0]!.value.event).toEqual(successes[1]!.value.event);
			expect(persisted.change_list.changes).toHaveLength(1);
			expect(persisted.evidence).toHaveLength(1);
			expect(decoder.decode(persisted.snapshot)).toBe("before");
			expect(persisted.payload_rows).toContainEqual(
				expect.objectContaining({
					expected: null,
					replacement: null,
					state: "consumed",
				}),
			);
			expect(decoder.decode(await readFile(join(root, "src", "example.ts")))).toBe("after");
			expect(controller.changed_results.value).toBe(0);
			expect(controller.finalization_attempts.value).toBe(2);
			expect(controller.replace_attempts.value).toBe(2);
			expect(controller.write_attempts.value).toBe(1);
		} finally {
			await Effect.runPromise(
				Deferred.succeed(preflight_gate.continue_first_read, undefined),
			);
			await Effect.runPromise(
				Deferred.succeed(preflight_gate.continue_second_read, undefined),
			);
			await Effect.runPromise(Deferred.succeed(preflight_gate.continue_publisher, undefined));
			await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
		}
	});

	it("converges synchronized committed cleanup across runtime instances", async () => {
		const { database_path, root } = await make_workspace();
		const controller = make_bounded_store_controller();
		const first_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_cleanup_first",
		);
		const second_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_cleanup_second",
		);
		const first_ready = await Effect.runPromise(Deferred.make<void>());
		const second_ready = await Effect.runPromise(Deferred.make<void>());
		const start = await Effect.runPromise(Deferred.make<void>());

		try {
			await first_runtime.runPromise(Read());
			await second_runtime.runPromise(Read());
			await first_runtime.runPromise(SeedBaseRun(root));
			await first_runtime.runPromise(InstallEvidenceFailure());

			const interrupted = await first_runtime.runPromise(Effect.exit(Replace()));

			await first_runtime.runPromise(RemoveEvidenceFailure());

			const first_pending = first_runtime.runPromise(
				Effect.exit(ReplaceAfterBarrier(first_ready, start)),
			);
			const second_pending = second_runtime.runPromise(
				Effect.exit(ReplaceAfterBarrier(second_ready, start)),
			);

			await Effect.runPromise(
				Effect.all([Deferred.await(first_ready), Deferred.await(second_ready)], {
					concurrency: "unbounded",
				}),
			);
			await Effect.runPromise(Deferred.succeed(start, undefined));

			const retries = await Promise.all([first_pending, second_pending]);
			const successes = retries.filter(Exit.isSuccess);
			const persisted = await first_runtime.runPromise(InspectCommittedReplacement("after"));

			expect(Exit.isFailure(interrupted)).toBe(true);
			expect(successes).toHaveLength(2);
			expect(successes.map((result) => result.value.status)).toEqual([
				"duplicate",
				"duplicate",
			]);
			expect(successes[0]!.value.event).toEqual(successes[1]!.value.event);
			expect(persisted.evidence).toHaveLength(1);
			expect(persisted.payload_rows).toContainEqual(
				expect.objectContaining({
					expected: null,
					replacement: null,
					state: "consumed",
				}),
			);
			expect(decoder.decode(await readFile(join(root, "src", "example.ts")))).toBe("after");
			expect(controller.finalization_attempts.value).toBe(1);
			expect(controller.replace_attempts.value).toBe(1);
			expect(controller.write_attempts.value).toBe(1);
		} finally {
			await Effect.runPromise(Deferred.succeed(start, undefined));
			await first_runtime.runPromise(RemoveEvidenceFailure()).catch(() => undefined);
			await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
		}
	});

	it("fails closed when thread erasure wins during the authority root proof", async () => {
		const { database_path, root } = await make_workspace();
		const authority_proof_gate: AuthorityProofGate = {
			continue_proof: await Effect.runPromise(Deferred.make<void>()),
			proof_started: await Effect.runPromise(Deferred.make<void>()),
		};
		const controller = make_bounded_store_controller();
		const replacing_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_erasure_replacing",
			authority_proof_gate,
		);
		const erasing_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_erasure_claiming",
		);

		try {
			await replacing_runtime.runPromise(SeedBaseRun(root));

			const pending = replacing_runtime.runPromise(Effect.exit(Replace()));

			await Effect.runPromise(Deferred.await(authority_proof_gate.proof_started));
			await erasing_runtime.runPromise(ClaimThreadErasure());
			await Effect.runPromise(
				Deferred.succeed(authority_proof_gate.continue_proof, undefined),
			);

			const result = await pending;
			const rows = await replacing_runtime.runPromise(InspectReplacementRows());

			expect(JSON.stringify(result)).toContain('"operation":"replace"');
			expect(JSON.stringify(result)).toContain('"reason":"failed"');
			expect(rows).toEqual({
				authorities: [],
				changes: [],
				evidence: [],
				operations: [],
				payloads: [],
				snapshots: [],
			});
			expect(decoder.decode(await readFile(join(root, "src", "example.ts")))).toBe("before");
			expect(controller.replace_attempts.value).toBe(0);
			expect(controller.write_attempts.value).toBe(0);
			expect(controller.finalization_attempts.value).toBe(0);
		} finally {
			await Effect.runPromise(
				Deferred.succeed(authority_proof_gate.continue_proof, undefined),
			);
			await Promise.all([replacing_runtime.dispose(), erasing_runtime.dispose()]);
		}
	});

	it("settles an exact committed retry after restart without reopening the terminal run", async () => {
		const { database_path, root } = await make_workspace();
		const controller = make_bounded_store_controller();
		const first_runtime = make_runtime(database_path, root, controller, "workspace_file_first");
		let accepted_event: unknown;

		try {
			await first_runtime.runPromise(SeedBaseRun(root));
			accepted_event = (await first_runtime.runPromise(Replace())).event;
			await first_runtime.runPromise(TerminalizeBaseRun());
		} finally {
			await first_runtime.dispose();
		}

		expect(controller.load_attempts.value).toBe(1);

		const second_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_second",
		);

		try {
			const duplicate = await second_runtime.runPromise(Replace());
			const persisted = await second_runtime.runPromise(InspectCommittedReplacement("after"));

			expect(duplicate).toEqual({ event: accepted_event, status: "duplicate" });
			expect(controller.replace_attempts.value).toBe(1);
			expect(controller.finalization_attempts.value).toBe(1);
			expect(controller.load_attempts.value).toBe(2);
			expect(persisted.evidence).toHaveLength(1);
			expect(persisted.payload_record_exists).toBe(true);
			expect(persisted.payload_available).toMatchObject({ _tag: "Failure" });
		} finally {
			await second_runtime.dispose();
		}
	});

	it("resumes finalization after publication without issuing another replacement", async () => {
		const { database_path, root } = await make_workspace();
		const controller = make_bounded_store_controller();
		const first_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_finalize_first",
		);

		try {
			controller.fail_next_finalization();
			await first_runtime.runPromise(SeedBaseRun(root));
			await expect(first_runtime.runPromise(Replace())).rejects.toMatchObject({
				operation: "replace",
				reason: "failed",
			});
			await first_runtime.runPromise(TerminalizeBaseRun());
		} finally {
			await first_runtime.dispose();
		}

		expect(controller.load_attempts.value).toBe(1);

		const second_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_finalize_second",
		);

		try {
			const result = await second_runtime.runPromise(Replace());
			const persisted = await second_runtime.runPromise(InspectCommittedReplacement("after"));

			expect(result.status).toBe("accepted");
			expect(controller.replace_attempts.value).toBe(1);
			expect(controller.finalization_attempts.value).toBe(2);
			expect(controller.load_attempts.value).toBe(2);
			expect(decoder.decode(await readFile(join(root, "src", "example.ts")))).toBe("after");
			expect(persisted.operation).toMatchObject({
				_tag: "Some",
				value: { lifecycle: "committed" },
			});
			expect(persisted.change_list.changes).toHaveLength(1);
			expect(persisted.evidence).toHaveLength(1);
			expect(persisted.payload_record_exists).toBe(true);
			expect(persisted.payload_available).toMatchObject({ _tag: "Failure" });
			expect(decoder.decode(persisted.snapshot)).toBe("before");
		} finally {
			await second_runtime.dispose();
		}
	});

	it("persists changed as terminal and keeps its exact retry side-effect free after restart", async () => {
		const { database_path, root } = await make_workspace();
		const controller = make_bounded_store_controller();
		const first_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_changed_first",
		);

		try {
			await first_runtime.runPromise(SeedBaseRun(root));
			await writeFile(join(root, "src", "example.ts"), "external");
			await expect(first_runtime.runPromise(Replace())).rejects.toMatchObject({
				operation: "replace",
				reason: "changed",
			});
			await first_runtime.runPromise(TerminalizeBaseRun());
		} finally {
			await first_runtime.dispose();
		}

		expect(controller.load_attempts.value).toBe(1);

		const second_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_changed_second",
		);

		try {
			await expect(second_runtime.runPromise(Replace())).rejects.toMatchObject({
				operation: "replace",
				reason: "changed",
			});
			const result = await second_runtime.runPromise(
				Effect.gen(function* () {
					const changes = yield* WorkspaceChangeRepository;
					const journal = yield* JournalStore;
					const snapshots = yield* WorkspaceSnapshotStore;

					return {
						change_list: yield* changes.List("thread_workspace_file", "workspace_file"),
						evidence: yield* journal.ReadCorrelatedEvents(
							"workspace_evidence:replace_message:correlation",
						),
						operation: yield* changes.ReadOperation("replace_message"),
						snapshot_state: yield* snapshots
							.Exists({
								change_id: "change_workspace_file",
								thread_id: "thread_workspace_file",
							})
							.pipe(Effect.exit),
					};
				}),
			);

			expect(controller.replace_attempts.value).toBe(0);
			expect(controller.finalization_attempts.value).toBe(0);
			expect(controller.load_attempts.value).toBe(2);
			expect(result.operation).toMatchObject({
				_tag: "Some",
				value: { lifecycle: "rejected" },
			});
			expect(result.change_list.changes).toEqual([]);
			expect(result.evidence).toEqual([]);
			expect(result.snapshot_state).toMatchObject({ _tag: "Failure" });
		} finally {
			await second_runtime.dispose();
		}
	});

	it("reviews and rolls back through the pinned source authority after its run terminates", async () => {
		const { database_path, root } = await make_workspace();
		const controller = make_bounded_store_controller();
		const first_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_rollback_first",
		);
		let rollback_event: unknown;

		try {
			await first_runtime.runPromise(SeedBaseRun(root));
			await first_runtime.runPromise(Replace());
			await first_runtime.runPromise(TerminalizeBaseRun());
			const reviewed = await first_runtime.runPromise(Review());
			const rolled_back = await first_runtime.runPromise(Rollback());
			const persisted = await first_runtime.runPromise(InspectRollback());

			rollback_event = rolled_back.event;

			expect(reviewed.status).toBe("accepted");
			expect(rolled_back.status).toBe("accepted");
			expect(decoder.decode(await readFile(join(root, "src", "example.ts")))).toBe("before");
			expect(persisted.operation).toMatchObject({
				_tag: "Some",
				value: { evidence_recorded: true, lifecycle: "committed" },
			});
			expect(persisted.change_list.changes).toMatchObject([
				{
					change_id: "change_workspace_file",
					review_state: "rolled_back",
					rollback_state: "consumed",
					version: 3,
				},
			]);
			expect(persisted.evidence).toHaveLength(1);
			expect(persisted.evidence[0]).toMatchObject({
				payload: {
					operation: "write",
					path: "src/example.ts",
					type: "filesystem.mutation",
				},
			});
			expect(persisted.evidence[0]?.agent_id).toBeUndefined();
			expect(persisted.evidence[0]?.run_id).toBeUndefined();
			expect(persisted.payload_record_state).toMatchObject({
				_tag: "Success",
				value: true,
			});
			expect(persisted.payload_available).toMatchObject({ _tag: "Failure" });
			expect(persisted.snapshot_available).toBe(false);
			expect(persisted.snapshot_state).toMatchObject({ _tag: "Failure" });
			expect(controller.replace_attempts.value).toBe(2);
			expect(controller.finalization_attempts.value).toBe(2);
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_rollback_second",
		);

		try {
			const duplicate = await second_runtime.runPromise(Rollback());
			const persisted = await second_runtime.runPromise(InspectRollback());

			expect(duplicate).toEqual({ event: rollback_event, status: "duplicate" });
			expect(controller.replace_attempts.value).toBe(2);
			expect(controller.finalization_attempts.value).toBe(2);
			expect(persisted.evidence).toHaveLength(1);
			expect(persisted.snapshot_available).toBe(false);
			expect(persisted.payload_available).toMatchObject({ _tag: "Failure" });
		} finally {
			await second_runtime.dispose();
		}

		expect(controller.load_attempts.value).toBe(2);
	});

	it("recovers an applied rollback after restart without replacing the file again", async () => {
		const { database_path, root } = await make_workspace();
		const controller = make_bounded_store_controller();
		const first_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_rollback_recovery_first",
		);

		try {
			await first_runtime.runPromise(SeedBaseRun(root));
			await first_runtime.runPromise(Replace());
			await first_runtime.runPromise(TerminalizeBaseRun());
			controller.fail_next_finalization();

			await expect(first_runtime.runPromise(Rollback())).rejects.toMatchObject({
				operation: "rollback",
				reason: "failed",
			});
			expect(decoder.decode(await readFile(join(root, "src", "example.ts")))).toBe("before");
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_rollback_recovery_second",
		);

		try {
			const recovered = await second_runtime.runPromise(Rollback());
			const persisted = await second_runtime.runPromise(InspectRollback());

			expect(recovered.status).toBe("accepted");
			expect(controller.replace_attempts.value).toBe(2);
			expect(controller.finalization_attempts.value).toBe(3);
			expect(persisted.operation).toMatchObject({
				_tag: "Some",
				value: { evidence_recorded: true, lifecycle: "committed" },
			});
			expect(persisted.change_list.changes).toMatchObject([
				{ review_state: "rolled_back", rollback_state: "consumed", version: 2 },
			]);
			expect(persisted.evidence).toHaveLength(1);
			expect(persisted.snapshot_available).toBe(false);
			expect(persisted.payload_available).toMatchObject({ _tag: "Failure" });
		} finally {
			await second_runtime.dispose();
		}

		expect(controller.load_attempts.value).toBe(2);
	});

	it("recovers committed rollback cleanup after snapshot consumption aborts", async () => {
		const { database_path, root } = await make_workspace();
		const controller = make_bounded_store_controller();
		const first_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_rollback_cleanup_first",
		);

		try {
			await first_runtime.runPromise(SeedBaseRun(root));
			await first_runtime.runPromise(Replace());
			await first_runtime.runPromise(TerminalizeBaseRun());
			await first_runtime.runPromise(InstallSnapshotConsumeFailure());

			try {
				await expect(first_runtime.runPromise(Rollback())).rejects.toMatchObject({
					operation: "rollback",
					reason: "failed",
				});
			} finally {
				await first_runtime.runPromise(RemoveSnapshotConsumeFailure());
			}

			const checkpoint = await first_runtime.runPromise(InspectRollback());

			expect(checkpoint.operation).toMatchObject({
				_tag: "Some",
				value: { evidence_recorded: false, lifecycle: "committed" },
			});
			expect(checkpoint.change_list.changes).toMatchObject([
				{ review_state: "rolled_back", rollback_state: "consumed", version: 2 },
			]);
			expect(checkpoint.evidence).toEqual([]);
			expect(checkpoint.snapshot_rows).toContainEqual(
				expect.objectContaining({
					change_id: "change_workspace_file",
					content: expect.any(Uint8Array),
					state: "available",
				}),
			);
			expect(checkpoint.payload_rows).toContainEqual(
				expect.objectContaining({
					expected: expect.any(Uint8Array),
					message_id: "rollback_message",
					state: "available",
				}),
			);
			expect(controller.replace_attempts.value).toBe(2);
			expect(controller.finalization_attempts.value).toBe(2);
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_rollback_cleanup_second",
		);

		try {
			const duplicate = await second_runtime.runPromise(Rollback());
			const settled = await second_runtime.runPromise(InspectRollback());

			expect(duplicate.status).toBe("duplicate");
			expect(controller.replace_attempts.value).toBe(2);
			expect(controller.finalization_attempts.value).toBe(2);
			expect(settled.operation).toMatchObject({
				_tag: "Some",
				value: { evidence_recorded: true, lifecycle: "committed" },
			});
			expect(settled.evidence).toHaveLength(1);
			expect(settled.snapshot_rows).toContainEqual(
				expect.objectContaining({
					change_id: "change_workspace_file",
					content: null,
					state: "consumed",
				}),
			);
			expect(settled.payload_rows).toContainEqual(
				expect.objectContaining({
					expected: null,
					message_id: "rollback_message",
					replacement: null,
					state: "consumed",
				}),
			);
		} finally {
			await second_runtime.dispose();
		}

		expect(controller.load_attempts.value).toBe(2);
	});

	it("keeps a store-rejected rollback terminal while erasing its staged payload", async () => {
		const { database_path, root } = await make_workspace();
		const controller = make_bounded_store_controller();
		const first_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_rollback_changed_first",
		);

		try {
			await first_runtime.runPromise(SeedBaseRun(root));
			await first_runtime.runPromise(Replace());
			await first_runtime.runPromise(TerminalizeBaseRun());
			controller.change_next_replacement();

			await expect(first_runtime.runPromise(Rollback())).rejects.toMatchObject({
				operation: "rollback",
				reason: "changed",
			});
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_rollback_changed_second",
		);

		try {
			await expect(second_runtime.runPromise(Rollback())).rejects.toMatchObject({
				operation: "rollback",
				reason: "changed",
			});
			const persisted = await second_runtime.runPromise(InspectRollback());

			expect(decoder.decode(await readFile(join(root, "src", "example.ts")))).toBe("after");
			expect(controller.replace_attempts.value).toBe(2);
			expect(controller.finalization_attempts.value).toBe(1);
			expect(persisted.operation).toMatchObject({
				_tag: "Some",
				value: { lifecycle: "rejected" },
			});
			expect(persisted.change_list.changes).toMatchObject([
				{ review_state: "needs_review", rollback_state: "available", version: 1 },
			]);
			expect(persisted.evidence).toEqual([]);
			expect(persisted.payload_record_state).toMatchObject({ _tag: "Failure" });
			expect(persisted.payload_available).toMatchObject({ _tag: "Failure" });
			expect(persisted.payload_rows).toContainEqual(
				expect.objectContaining({
					expected: null,
					expected_byte_count: null,
					expected_hash: null,
					message_id: "rollback_message",
					replacement: null,
					replacement_byte_count: null,
					replacement_hash: null,
					state: "consumed",
				}),
			);
			expect(persisted.snapshot_available).toBe(true);
			expect(persisted.snapshot_state).toMatchObject({ _tag: "Success" });
		} finally {
			await second_runtime.dispose();
		}

		expect(controller.load_attempts.value).toBe(2);
	});

	it("rejects mutation through the default empty bounded registry", async () => {
		const { database_path, root } = await make_workspace();
		const runtime = make_runtime(database_path, root);

		try {
			await runtime.runPromise(SeedBaseRun(root));
			await expect(runtime.runPromise(Read())).rejects.toMatchObject({
				operation: "read",
				reason: "failed",
			});
			await expect(runtime.runPromise(Replace())).rejects.toMatchObject({
				operation: "replace",
				reason: "failed",
			});

			expect(decoder.decode(await readFile(join(root, "src", "example.ts")))).toBe("before");
		} finally {
			await runtime.dispose();
		}
	});
});
