import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, Layer, Redacted } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { ContentIdentity } from "@artisan/protocol";
import {
	make_backend_runtime,
	type NativeBoundedRegularFileStoreOptions,
	ProtocolRouter,
	ThreadRetentionScheduler,
	WorkspaceChangeRepository,
	WorkspaceFileService,
	WorkspaceMutationPayloadStore,
	WorkspaceSnapshotStore,
} from "@artisan/backend";

import { make_workspace_bounded_regular_file_store_registry_layer } from "../../modules/backend/src/filesystem/workspace-bounded-regular-file-store-registry";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import {
	OrchestrationCoordinators,
	OrchestrationRuns,
	WorkspaceChangeSnapshots,
	WorkspaceMutationPayloads,
} from "../../modules/backend/src/persistence/schema";
import { Database } from "../../modules/backend/src/persistence/database";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const receipt_authentication_key = Redacted.make(new Uint8Array(32).fill(9));
const now = "2026-07-12T13:00:00.000Z";
const encoder = new TextEncoder();
const decoder = new TextDecoder();

type NativeController = {
	readonly change_next_replacement: () => void;
	readonly close_attempts: { value: number };
	readonly finalization_attempts: { value: number };
	readonly load_attempts: { value: number };
	readonly replace_attempts: { value: number };
	readonly fail_next_finalization: () => void;
	readonly load_native_module: NonNullable<
		NativeBoundedRegularFileStoreOptions["load_native_module"]
	>;
};

type NativeReplacementOptions = {
	readonly expected: Uint8Array;
	readonly maximumBytes: number;
	readonly operationId: string;
	readonly path: string;
	readonly replacement: Uint8Array;
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
	left: NativeReplacementOptions,
	right: NativeReplacementOptions,
) {
	return (
		left.maximumBytes === right.maximumBytes &&
		left.operationId === right.operationId &&
		left.path === right.path &&
		bytes_match(left.expected, right.expected) &&
		bytes_match(left.replacement, right.replacement)
	);
}

function make_native_controller(): NativeController {
	const close_attempts = { value: 0 };
	const finalization_attempts = { value: 0 };
	const load_attempts = { value: 0 };
	const replace_attempts = { value: 0 };
	const receipts = new Map<string, NativeReplacementOptions>();
	let remaining_changed_results = 0;
	let remaining_finalization_failures = 0;

	class FakeNativeBoundedRegularFileStore {
		constructor(
			readonly root: string,
			_receipt_authentication_key: Uint8Array,
		) {}

		authorizeRoot(candidate_root: string) {
			return Promise.resolve(candidate_root === this.root);
		}

		close() {
			close_attempts.value += 1;
		}

		async finalizeRegularFileReplacement(options: NativeReplacementOptions) {
			finalization_attempts.value += 1;

			if (remaining_finalization_failures > 0) {
				remaining_finalization_failures -= 1;

				throw new Error("deterministic finalization failure");
			}

			const receipt = receipts.get(options.operationId);

			if (receipt === undefined || !replacement_options_match(receipt, options)) {
				throw new Error("replacement receipt intent changed");
			}

			receipts.delete(options.operationId);
		}

		async readRegularFile(path: string, maximum_bytes: number) {
			const bytes = new Uint8Array(await readFile(join(this.root, path)));

			if (bytes.byteLength > maximum_bytes) throw new Error("file exceeds maximum");

			return bytes;
		}

		async replaceRegularFile(options: NativeReplacementOptions) {
			replace_attempts.value += 1;
			const receipt = receipts.get(options.operationId);

			if (receipt !== undefined) {
				if (!replacement_options_match(receipt, options)) {
					throw new Error("replacement operation intent changed");
				}

				return "AlreadyReplaced";
			}

			if (remaining_changed_results > 0) {
				remaining_changed_results -= 1;

				return "Changed";
			}

			const target = join(this.root, options.path);
			const current = new Uint8Array(await readFile(target));
			const matches =
				current.byteLength === options.expected.byteLength &&
				current.every((value, index) => value === options.expected[index]);

			if (!matches || options.replacement.byteLength > options.maximumBytes) return "Changed";

			await writeFile(target, options.replacement);
			receipts.set(options.operationId, {
				...options,
				expected: new Uint8Array(options.expected),
				replacement: new Uint8Array(options.replacement),
			});

			return "Replaced";
		}
	}

	return {
		change_next_replacement: () => {
			remaining_changed_results += 1;
		},
		close_attempts,
		finalization_attempts,
		load_attempts,
		replace_attempts,
		fail_next_finalization: () => {
			remaining_finalization_failures += 1;
		},
		load_native_module: () => {
			load_attempts.value += 1;

			return {
				NativeBoundedRegularFileStore: FakeNativeBoundedRegularFileStore,
				getNativeBuildDescriptor: () => ({
					architecture: "x86_64",
					operatingSystem: "windows",
					target: "x86_64-pc-windows-msvc",
					testHooksEnabled: false,
				}),
			};
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
	controller?: NativeController,
	instance_id = "workspace_file_service_composition",
) {
	return make_backend_runtime({
		database_path,
		migrations_path,
		retention_scheduler: make_inert_scheduler_layer(),
		runtime_metadata: make_metadata_layer(instance_id),
		...(controller === undefined
			? {}
			: {
					workspace_bounded_regular_file_store_registry:
						make_workspace_bounded_regular_file_store_registry_layer(
							[{ root, workspace_id: "workspace_file" }],
							{
								load_native_module: controller.load_native_module,
								receipt_authentication_key,
							},
						).pipe(Layer.provide(NodeFileSystem.layer)),
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

function Replace(input = replacement_input()) {
	return Effect.gen(function* () {
		const service = yield* WorkspaceFileService;

		return yield* service.Replace(input);
	});
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
			snapshot: yield* snapshots.Read({
				change_id: input.change_id,
				expected_identity: input.expected_before,
				thread_id: input.thread_id,
			}),
		};
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
		const controller = make_native_controller();
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

		expect(controller.close_attempts.value).toBe(1);
	});

	it("settles an exact committed retry after restart without reopening the terminal run", async () => {
		const { database_path, root } = await make_workspace();
		const controller = make_native_controller();
		const first_runtime = make_runtime(database_path, root, controller, "workspace_file_first");
		let accepted_event: unknown;

		try {
			await first_runtime.runPromise(SeedBaseRun(root));
			accepted_event = (await first_runtime.runPromise(Replace())).event;
			await first_runtime.runPromise(TerminalizeBaseRun());
		} finally {
			await first_runtime.dispose();
		}

		expect(controller.close_attempts.value).toBe(1);
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

		expect(controller.close_attempts.value).toBe(2);
	});

	it("resumes finalization after native publication without issuing another replacement", async () => {
		const { database_path, root } = await make_workspace();
		const controller = make_native_controller();
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

		expect(controller.close_attempts.value).toBe(1);
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

		expect(controller.close_attempts.value).toBe(2);
	});

	it("persists changed as terminal and keeps its exact retry side-effect free after restart", async () => {
		const { database_path, root } = await make_workspace();
		const controller = make_native_controller();
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

		expect(controller.close_attempts.value).toBe(1);
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

		expect(controller.close_attempts.value).toBe(2);
	});

	it("reviews and rolls back through the pinned source authority after its run terminates", async () => {
		const { database_path, root } = await make_workspace();
		const controller = make_native_controller();
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

		expect(controller.close_attempts.value).toBe(2);
		expect(controller.load_attempts.value).toBe(2);
	});

	it("recovers an applied rollback after restart without replacing the file again", async () => {
		const { database_path, root } = await make_workspace();
		const controller = make_native_controller();
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

		expect(controller.close_attempts.value).toBe(2);
		expect(controller.load_attempts.value).toBe(2);
	});

	it("recovers committed rollback cleanup after snapshot consumption aborts", async () => {
		const { database_path, root } = await make_workspace();
		const controller = make_native_controller();
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

		expect(controller.close_attempts.value).toBe(2);
		expect(controller.load_attempts.value).toBe(2);
	});

	it("keeps a native-rejected rollback terminal while erasing its staged payload", async () => {
		const { database_path, root } = await make_workspace();
		const controller = make_native_controller();
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

		expect(controller.close_attempts.value).toBe(2);
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
