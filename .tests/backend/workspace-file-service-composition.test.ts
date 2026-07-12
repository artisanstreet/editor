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
	ProtocolRouter,
	WorkspaceChangeRepository,
	WorkspaceFileService,
	WorkspaceMutationPayloadStore,
	WorkspaceSnapshotStore,
	ThreadRetentionScheduler,
} from "@artisan/backend";

import { make_workspace_bounded_regular_file_store_registry_layer } from "../../modules/backend/src/filesystem/workspace-bounded-regular-file-store-registry";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import {
	OrchestrationCoordinators,
	OrchestrationRuns,
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
	readonly finalization_attempts: { value: number };
	readonly load_attempts: { value: number };
	readonly replace_attempts: { value: number };
	readonly fail_next_finalization: () => void;
	readonly load_native_module: () => unknown;
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

function make_native_controller(): NativeController {
	const finalization_attempts = { value: 0 };
	const load_attempts = { value: 0 };
	const replace_attempts = { value: 0 };
	const receipts = new Set<string>();
	let remaining_finalization_failures = 0;

	class FakeNativeBoundedRegularFileStore {
		constructor(
			readonly root: string,
			_receipt_authentication_key: Uint8Array,
		) {}

		authorizeRoot(candidate_root: string) {
			return Promise.resolve(candidate_root === this.root);
		}

		close() {}

		async finalizeRegularFileReplacement(options: { readonly operationId: string }) {
			finalization_attempts.value += 1;

			if (remaining_finalization_failures > 0) {
				remaining_finalization_failures -= 1;

				throw new Error("deterministic finalization failure");
			}

			receipts.delete(options.operationId);
		}

		async readRegularFile(path: string, maximum_bytes: number) {
			const bytes = new Uint8Array(await readFile(join(this.root, path)));

			if (bytes.byteLength > maximum_bytes) throw new Error("file exceeds maximum");

			return bytes;
		}

		async replaceRegularFile(options: {
			readonly expected: Uint8Array;
			readonly maximumBytes: number;
			readonly operationId: string;
			readonly path: string;
			readonly replacement: Uint8Array;
		}) {
			replace_attempts.value += 1;

			if (receipts.has(options.operationId)) return "AlreadyReplaced";

			const target = join(this.root, options.path);
			const current = new Uint8Array(await readFile(target));
			const matches =
				current.byteLength === options.expected.byteLength &&
				current.every((value, index) => value === options.expected[index]);

			if (!matches || options.replacement.byteLength > options.maximumBytes) return "Changed";

			await writeFile(target, options.replacement);
			receipts.add(options.operationId);

			return "Replaced";
		}
	}

	return {
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

function Replace(input = replacement_input()) {
	return Effect.gen(function* () {
		const service = yield* WorkspaceFileService;

		return yield* service.Replace(input);
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
		} finally {
			await runtime.dispose();
		}
	});

	it("settles an exact committed retry after restart without reopening the terminal run", async () => {
		const { database_path, root } = await make_workspace();
		const controller = make_native_controller();
		const first_runtime = make_runtime(database_path, root, controller, "workspace_file_first");

		await first_runtime.runPromise(SeedBaseRun(root));
		const accepted = await first_runtime.runPromise(Replace());
		await first_runtime.runPromise(TerminalizeBaseRun());
		await first_runtime.dispose();

		const second_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_second",
		);

		try {
			const duplicate = await second_runtime.runPromise(Replace());
			const persisted = await second_runtime.runPromise(InspectCommittedReplacement("after"));

			expect(duplicate).toEqual({ event: accepted.event, status: "duplicate" });
			expect(controller.replace_attempts.value).toBe(1);
			expect(controller.finalization_attempts.value).toBe(1);
			expect(persisted.evidence).toHaveLength(1);
			expect(persisted.payload_record_exists).toBe(true);
			expect(persisted.payload_available).toMatchObject({ _tag: "Failure" });
		} finally {
			await second_runtime.dispose();
		}
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

		controller.fail_next_finalization();
		await first_runtime.runPromise(SeedBaseRun(root));
		await expect(first_runtime.runPromise(Replace())).rejects.toMatchObject({
			operation: "replace",
			reason: "failed",
		});
		await first_runtime.dispose();

		const second_runtime = make_runtime(
			database_path,
			root,
			controller,
			"workspace_file_finalize_second",
		);

		try {
			await second_runtime.runPromise(Replace());

			expect(controller.replace_attempts.value).toBe(1);
			expect(controller.finalization_attempts.value).toBe(2);
			expect(decoder.decode(await readFile(join(root, "src", "example.ts")))).toBe("after");
		} finally {
			await second_runtime.dispose();
		}
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

		await first_runtime.runPromise(SeedBaseRun(root));
		await writeFile(join(root, "src", "example.ts"), "external");
		await expect(first_runtime.runPromise(Replace())).rejects.toMatchObject({
			operation: "replace",
			reason: "changed",
		});
		await first_runtime.dispose();

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

					return {
						evidence: yield* journal.ReadCorrelatedEvents(
							"workspace_evidence:replace_message:correlation",
						),
						operation: yield* changes.ReadOperation("replace_message"),
					};
				}),
			);

			expect(controller.replace_attempts.value).toBe(0);
			expect(result.operation).toMatchObject({
				_tag: "Some",
				value: { lifecycle: "rejected" },
			});
			expect(result.evidence).toEqual([]);
		} finally {
			await second_runtime.dispose();
		}
	});

	it("rejects mutation through the default empty bounded registry", async () => {
		const { database_path, root } = await make_workspace();
		const runtime = make_runtime(database_path, root);

		try {
			await runtime.runPromise(SeedBaseRun(root));
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
