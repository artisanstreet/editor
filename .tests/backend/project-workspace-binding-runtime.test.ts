import { createHash } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Deferred, Effect, Layer, Ref } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { ContentIdentity } from "@artisan/protocol";

import { BoundedRegularFileStore } from "../../modules/backend/src/filesystem/bounded-regular-file-store";
import { WorkspaceBoundedRegularFileStoreRegistry } from "../../modules/backend/src/filesystem/workspace-bounded-regular-file-store-registry";
import { WorkspaceFilesystemRegistry } from "../../modules/backend/src/filesystem/workspace-filesystem-registry";
import { Database } from "../../modules/backend/src/persistence/database";
import {
	OrchestrationCoordinators,
	OrchestrationRuns,
} from "../../modules/backend/src/persistence/tables";
import { ProtocolRouter } from "../../modules/backend/src/protocol/router";
import { make_backend_runtime } from "../../modules/backend/src/runtime/backend-runtime";
import { ArtisanToolCapabilityState } from "../../modules/backend/src/tools/artisan-tool-registry";
import { ExecuteTool } from "../../modules/backend/src/tools/tool-handlers";
import { WorkspaceFileDiscovery } from "../../modules/backend/src/workspace/files/discovery";
import { WorkspaceFileService } from "../../modules/backend/src/workspace/files/service";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const now = "2026-07-12T13:00:00.000Z";
const workspace_id = "workspace_runtime";
const encoder = new TextEncoder();

const MakeDatabasePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-workspace-binding-runtime-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};

type RegistryOperation = "authorize" | "get" | "list";

interface RegistryControls {
	readonly active_operations: Ref.Ref<number>;
	readonly constructions: Ref.Ref<number>;
	readonly operation_instances: Ref.Ref<ReadonlyArray<number>>;
	readonly operation_names: Ref.Ref<ReadonlyArray<RegistryOperation>>;
	readonly operation_target: number;
	readonly operations: Ref.Ref<number>;
	readonly operations_entered: Deferred.Deferred<void>;
	readonly peak_operations: Ref.Ref<number>;
	readonly reconciled_instances: Ref.Ref<ReadonlyArray<number>>;
	readonly reconciliations: Ref.Ref<number>;
	readonly release_operations: Deferred.Deferred<void>;
}

interface Controls {
	readonly bounded: RegistryControls;
	readonly bounded_authorizations: Ref.Ref<
		ReadonlyArray<{ readonly working_directory: string; readonly workspace_id: string }>
	>;
	readonly binding_entered: Deferred.Deferred<void>;
	readonly filesystem: RegistryControls;
	readonly release_binding: Deferred.Deferred<void>;
	readonly store_finalizations: Ref.Ref<number>;
	readonly store_reads: Ref.Ref<number>;
	readonly store_replacements: Ref.Ref<number>;
}

const MakeRegistryControls = (operation_target: number) =>
	Effect.all({
		active_operations: Ref.make(0),
		constructions: Ref.make(0),
		operation_instances: Ref.make<ReadonlyArray<number>>([]),
		operation_names: Ref.make<ReadonlyArray<RegistryOperation>>([]),
		operations: Ref.make(0),
		operations_entered: Deferred.make<void>(),
		peak_operations: Ref.make(0),
		reconciled_instances: Ref.make<ReadonlyArray<number>>([]),
		reconciliations: Ref.make(0),
		release_operations: Deferred.make<void>(),
	}).pipe(Effect.map((controls) => ({ ...controls, operation_target })));

const MakeControls = () =>
	Effect.all({
		bounded: MakeRegistryControls(6),
		bounded_authorizations: Ref.make<
			ReadonlyArray<{ readonly working_directory: string; readonly workspace_id: string }>
		>([]),
		binding_entered: Deferred.make<void>(),
		filesystem: MakeRegistryControls(3),
		release_binding: Deferred.make<void>(),
		store_finalizations: Ref.make(0),
		store_reads: Ref.make(0),
		store_replacements: Ref.make(0),
	});

const AwaitRegistryOperation = (
	controls: RegistryControls,
	reconciled: Ref.Ref<boolean>,
	instance_id: number,
	operation: RegistryOperation,
) =>
	Effect.gen(function* () {
		if (!(yield* Ref.get(reconciled))) {
			return yield* Effect.die(
				new Error(`registry instance ${instance_id} was used before it was reconciled`),
			);
		}

		yield* Ref.update(controls.operation_instances, (instances) => [...instances, instance_id]);
		yield* Ref.update(controls.operation_names, (operations) => [...operations, operation]);
		const operation_count = yield* Ref.updateAndGet(controls.operations, (count) => count + 1);
		const active_count = yield* Ref.updateAndGet(
			controls.active_operations,
			(count) => count + 1,
		);
		yield* Ref.update(controls.peak_operations, (peak) => Math.max(peak, active_count));

		if (operation_count === controls.operation_target) {
			yield* Deferred.succeed(controls.operations_entered, undefined);
		}

		yield* Deferred.await(controls.release_operations).pipe(
			Effect.ensuring(Ref.update(controls.active_operations, (count) => count - 1)),
		);
	});

const MakeFilesystemRegistry = (controls: Controls, binding_fails = false) => {
	return Layer.effect(
		WorkspaceFilesystemRegistry,
		Effect.gen(function* () {
			const instance_id = yield* Ref.updateAndGet(
				controls.filesystem.constructions,
				(count) => count + 1,
			);
			const reconciled = yield* Ref.make(false);
			const filesystem = { List: () => Effect.succeed([]) };
			const AwaitOperation = (operation: RegistryOperation) =>
				AwaitRegistryOperation(controls.filesystem, reconciled, instance_id, operation);

			return {
				Authorize: ({ workspace_id }: { readonly workspace_id: string }) =>
					AwaitOperation("authorize").pipe(Effect.as({ filesystem, workspace_id })),
				Get: (requested_workspace_id: string) =>
					AwaitOperation("get").pipe(
						Effect.as({ filesystem, workspace_id: requested_workspace_id }),
					),
				ListWorkspaceIds: AwaitOperation("list").pipe(Effect.as([workspace_id])),
				Reconcile: () =>
					Effect.gen(function* () {
						yield* Ref.update(
							controls.filesystem.reconciliations,
							(count) => count + 1,
						);
						yield* Deferred.succeed(controls.binding_entered, undefined);
						yield* Deferred.await(controls.release_binding);

						if (binding_fails) {
							return yield* Effect.fail(new Error("binding unavailable"));
						}

						yield* Ref.set(reconciled, true);
						yield* Ref.update(controls.filesystem.reconciled_instances, (instances) => [
							...instances,
							instance_id,
						]);
						return [];
					}),
				Register: () => Effect.die("not used"),
			} as never;
		}),
	);
};

const MakeBoundedRegistry = (controls: Controls) =>
	Layer.effect(
		WorkspaceBoundedRegularFileStoreRegistry,
		Effect.gen(function* () {
			const instance_id = yield* Ref.updateAndGet(
				controls.bounded.constructions,
				(count) => count + 1,
			);
			const reconciled = yield* Ref.make(false);
			const before = encoder.encode("before");
			const store: typeof BoundedRegularFileStore.Service = {
				FinalizeRegularFileReplacement: () =>
					Ref.update(controls.store_finalizations, (count) => count + 1),
				ReadRegularFile: () =>
					Ref.update(controls.store_reads, (count) => count + 1).pipe(Effect.as(before)),
				ReplaceRegularFile: () =>
					Ref.update(controls.store_replacements, (count) => count + 1).pipe(
						Effect.as({ _tag: "Replaced" as const }),
					),
			};
			const reader = { ReadRegularFile: store.ReadRegularFile };
			const AwaitOperation = (operation: RegistryOperation) =>
				AwaitRegistryOperation(controls.bounded, reconciled, instance_id, operation);

			return {
				Authorize: (input: {
					readonly working_directory: string;
					readonly workspace_id: string;
				}) =>
					Ref.update(controls.bounded_authorizations, (authorizations) => [
						...authorizations,
						input,
					]).pipe(
						Effect.andThen(AwaitOperation("authorize")),
						Effect.as({ store, workspace_id: input.workspace_id }),
					),
				Get: (requested_workspace_id: string) =>
					AwaitOperation("get").pipe(
						Effect.as({ reader, workspace_id: requested_workspace_id }),
					),
				ListWorkspaceIds: AwaitOperation("list").pipe(Effect.as([workspace_id])),
				Reconcile: () =>
					Ref.update(controls.bounded.reconciliations, (count) => count + 1).pipe(
						Effect.andThen(Ref.set(reconciled, true)),
						Effect.andThen(
							Ref.update(controls.bounded.reconciled_instances, (instances) => [
								...instances,
								instance_id,
							]),
						),
						Effect.as([]),
					),
			} as never;
		}),
	);

function content_identity(content: string): ContentIdentity {
	const bytes = encoder.encode(content);

	return {
		algorithm: "sha256",
		byte_count: bytes.byteLength,
		content_hash: createHash("sha256").update(bytes).digest("hex"),
	};
}

function create_thread_command() {
	return {
		kind: "command" as const,
		message_id: "create_workspace_binding_thread",
		origin: "frontend" as const,
		payload: {
			title: "Workspace binding runtime",
			type: "thread.create" as const,
		},
		protocol_version: 1 as const,
		schema_version: 1 as const,
		sent_at: now,
		thread_id: "thread_workspace_binding",
	};
}

function replacement_input() {
	return {
		agent_id: "agent_workspace_binding",
		change_id: "change_workspace_binding",
		content: "after",
		expected_before: content_identity("before"),
		message_id: "replace_workspace_binding",
		path: "src/example.ts",
		run_id: "run_workspace_binding",
		sent_at: now,
		thread_id: "thread_workspace_binding",
		workspace_id,
	};
}

function SeedBaseRun(working_directory: string) {
	return Effect.gen(function* () {
		const database = yield* Database;
		const router = yield* ProtocolRouter;

		yield* router.Route(create_thread_command());
		yield* database.client.insert(OrchestrationCoordinators).values({
			active_run_id: "run_workspace_binding",
			agent_id: "agent_workspace_binding",
			created_at: now,
			display_name: "Coordinator",
			engine_id: "engine_workspace_binding",
			role: "primary",
			thread_id: "thread_workspace_binding",
			updated_at: now,
		});
		yield* database.client.insert(OrchestrationRuns).values({
			agent_id: "agent_workspace_binding",
			created_at: now,
			engine_id: "engine_workspace_binding",
			run_id: "run_workspace_binding",
			status: "running",
			thread_id: "thread_workspace_binding",
			updated_at: now,
			working_directory,
		});
	});
}

const ReleaseControls = (controls: Controls) =>
	Deferred.succeed(controls.release_binding, undefined).pipe(
		Effect.andThen(Deferred.succeed(controls.filesystem.release_operations, undefined)),
		Effect.andThen(Deferred.succeed(controls.bounded.release_operations, undefined)),
	);

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("project workspace binding runtime integration", () => {
	it("constructs while binding is held, then gates both discovery and terminal authorization", async () => {
		const controls = await Effect.runPromise(MakeControls());
		const runtime = make_backend_runtime({
			database_path: await MakeDatabasePath(),
			migrations_path,
			workspace_bounded_regular_file_store_registry: MakeBoundedRegistry(controls),
			workspace_filesystem_registry: MakeFilesystemRegistry(controls),
		});

		try {
			const services = await runtime.runPromise(
				Effect.all({
					discovery: Effect.service(WorkspaceFileDiscovery),
					registry: Effect.service(WorkspaceFilesystemRegistry),
					tools: Effect.service(ExecuteTool),
				}),
			);
			await Effect.runPromise(Deferred.await(controls.binding_entered));
			expect(await Effect.runPromise(Ref.get(controls.filesystem.constructions))).toBe(1);
			expect(await Effect.runPromise(Ref.get(controls.bounded.constructions))).toBe(1);
			expect(await Effect.runPromise(Ref.get(controls.filesystem.reconciliations))).toBe(1);
			expect(await Effect.runPromise(Ref.get(controls.bounded.reconciliations))).toBe(0);
			expect(
				await Effect.runPromise(Ref.get(controls.filesystem.reconciled_instances)),
			).toEqual([]);

			const discovery = runtime.runPromise(services.discovery.Discover({ workspace_id }));
			const workspace_ids = runtime.runPromise(services.registry.ListWorkspaceIds);
			const terminal = runtime.runPromise(
				services.tools.Execute({
					input: {
						args: [],
						cols: 80,
						executable: "node",
						rows: 24,
						terminal_id: "terminal_runtime",
						tool_id: "terminal.open",
						working_directory: "workspace",
						workspace_id,
					},
					invocation_id: "binding_runtime_terminal",
					thread_id: "thread_runtime",
				}),
			);
			await Effect.runPromise(Effect.yieldNow);
			expect(await Effect.runPromise(Ref.get(controls.filesystem.operations))).toBe(0);

			await Effect.runPromise(Deferred.succeed(controls.release_binding, undefined));
			await Effect.runPromise(Deferred.await(controls.filesystem.operations_entered));
			expect(await Effect.runPromise(Ref.get(controls.filesystem.operations))).toBe(3);
			expect(await Effect.runPromise(Ref.get(controls.filesystem.constructions))).toBe(1);
			expect(await Effect.runPromise(Ref.get(controls.filesystem.active_operations))).toBe(3);
			expect(await Effect.runPromise(Ref.get(controls.filesystem.peak_operations))).toBe(3);
			expect(
				await Effect.runPromise(Ref.get(controls.filesystem.operation_instances)),
			).toEqual([1, 1, 1]);
			expect(
				(await Effect.runPromise(Ref.get(controls.filesystem.operation_names))).toSorted(),
			).toEqual(["authorize", "get", "list"]);
			expect(
				await Effect.runPromise(Ref.get(controls.filesystem.reconciled_instances)),
			).toEqual([1]);
			expect(await Effect.runPromise(Ref.get(controls.bounded.constructions))).toBe(1);
			expect(await Effect.runPromise(Ref.get(controls.bounded.reconciliations))).toBe(1);
			expect(await Effect.runPromise(Ref.get(controls.bounded.reconciled_instances))).toEqual(
				[1],
			);
			await Effect.runPromise(
				Deferred.succeed(controls.filesystem.release_operations, undefined),
			);

			await expect(discovery).resolves.toMatchObject({ workspace_id });
			await expect(workspace_ids).resolves.toEqual([workspace_id]);
			await expect(terminal).resolves.toMatchObject({ status: "failed" });
			expect(await Effect.runPromise(Ref.get(controls.filesystem.active_operations))).toBe(0);
		} finally {
			await Effect.runPromise(ReleaseControls(controls));
			await runtime.dispose();
		}
	});

	it("gates direct bounded access, file authority, and built-in capability on one reconciled instance", async () => {
		const controls = await Effect.runPromise(MakeControls());
		const runtime = make_backend_runtime({
			database_path: await MakeDatabasePath(),
			migrations_path,
			workspace_bounded_regular_file_store_registry: MakeBoundedRegistry(controls),
			workspace_filesystem_registry: MakeFilesystemRegistry(controls),
		});

		try {
			const services = await runtime.runPromise(
				Effect.all({
					capabilities: Effect.service(ArtisanToolCapabilityState),
					files: Effect.service(WorkspaceFileService),
					registry: Effect.service(WorkspaceBoundedRegularFileStoreRegistry),
				}),
			);
			await Effect.runPromise(Deferred.await(controls.binding_entered));
			await runtime.runPromise(SeedBaseRun("workspace"));

			expect(await Effect.runPromise(Ref.get(controls.filesystem.constructions))).toBe(1);
			expect(await Effect.runPromise(Ref.get(controls.bounded.constructions))).toBe(1);
			expect(await Effect.runPromise(Ref.get(controls.filesystem.reconciliations))).toBe(1);
			expect(await Effect.runPromise(Ref.get(controls.bounded.reconciliations))).toBe(0);

			const direct_get = runtime.runPromise(services.registry.Get(workspace_id));
			const direct_authorize = runtime.runPromise(
				services.registry.Authorize({ working_directory: "workspace", workspace_id }),
			);
			const direct_list = runtime.runPromise(services.registry.ListWorkspaceIds);
			const read = runtime.runPromise(
				services.files.Read({ path: "src/example.ts", workspace_id }),
			);
			const replace = runtime.runPromise(services.files.Replace(replacement_input()));
			const capability = runtime.runPromise(
				services.capabilities.Get("workspace.file.read", workspace_id),
			);
			await Effect.runPromise(Effect.yieldNow);

			expect(await Effect.runPromise(Ref.get(controls.bounded.operations))).toBe(0);
			await Effect.runPromise(Deferred.succeed(controls.release_binding, undefined));
			await Effect.runPromise(Deferred.await(controls.bounded.operations_entered));

			expect(await Effect.runPromise(Ref.get(controls.bounded.reconciliations))).toBe(1);
			expect(await Effect.runPromise(Ref.get(controls.bounded.constructions))).toBe(1);
			expect(await Effect.runPromise(Ref.get(controls.bounded.reconciled_instances))).toEqual(
				[1],
			);
			expect(await Effect.runPromise(Ref.get(controls.bounded.operations))).toBe(6);
			expect(await Effect.runPromise(Ref.get(controls.bounded.active_operations))).toBe(6);
			expect(await Effect.runPromise(Ref.get(controls.bounded.peak_operations))).toBe(6);
			expect(await Effect.runPromise(Ref.get(controls.bounded.operation_instances))).toEqual([
				1, 1, 1, 1, 1, 1,
			]);
			expect(
				(await Effect.runPromise(Ref.get(controls.bounded.operation_names))).toSorted(),
			).toEqual(["authorize", "authorize", "get", "get", "get", "list"]);
			expect(await Effect.runPromise(Ref.get(controls.bounded_authorizations))).toEqual([
				{ working_directory: "workspace", workspace_id },
				{ working_directory: "workspace", workspace_id },
			]);

			await Effect.runPromise(
				Deferred.succeed(controls.bounded.release_operations, undefined),
			);
			const [
				get_result,
				authorize_result,
				list_result,
				read_result,
				replace_result,
				capability_result,
			] = await Promise.all([
				direct_get,
				direct_authorize,
				direct_list,
				read,
				replace,
				capability,
			]);

			expect(get_result).toMatchObject({ workspace_id });
			expect(authorize_result).toMatchObject({ workspace_id });
			expect(list_result).toEqual([workspace_id]);
			expect(read_result).toMatchObject({
				content: "before",
				identity: content_identity("before"),
				path: "src/example.ts",
				workspace_id,
			});
			expect(replace_result).toMatchObject({ status: "accepted" });
			expect(capability_result).toEqual({
				state: "available",
				tool_id: "workspace.file.read",
			});
			expect(await Effect.runPromise(Ref.get(controls.store_reads))).toBe(2);
			expect(await Effect.runPromise(Ref.get(controls.store_replacements))).toBe(1);
			expect(await Effect.runPromise(Ref.get(controls.store_finalizations))).toBe(1);
			expect(await Effect.runPromise(Ref.get(controls.bounded.active_operations))).toBe(0);
		} finally {
			await Effect.runPromise(ReleaseControls(controls));
			await runtime.dispose();
		}
	});

	it("maps a failed binding to the existing renderer-safe workspace outcome", async () => {
		const controls = await Effect.runPromise(MakeControls());
		const runtime = make_backend_runtime({
			database_path: await MakeDatabasePath(),
			migrations_path,
			workspace_bounded_regular_file_store_registry: MakeBoundedRegistry(controls),
			workspace_filesystem_registry: MakeFilesystemRegistry(controls, true),
		});

		try {
			const discovery = await runtime.runPromise(Effect.service(WorkspaceFileDiscovery));
			await Effect.runPromise(Deferred.await(controls.binding_entered));
			expect(await Effect.runPromise(Ref.get(controls.filesystem.constructions))).toBe(1);
			expect(await Effect.runPromise(Ref.get(controls.filesystem.reconciliations))).toBe(1);
			await Effect.runPromise(Deferred.succeed(controls.release_binding, undefined));
			await expect(
				runtime.runPromise(discovery.Discover({ workspace_id })),
			).rejects.toMatchObject({ reason: "unavailable" });
			expect(await Effect.runPromise(Ref.get(controls.filesystem.operations))).toBe(0);
		} finally {
			await Effect.runPromise(ReleaseControls(controls));
			await runtime.dispose();
		}
	});
});
