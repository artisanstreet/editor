import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, Fiber, Layer, Redacted, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	make_backend_runtime,
	make_workspace_bounded_regular_file_store_registry_layer,
	type NativeBoundedRegularFileStoreOptions,
	ProtocolRouter,
	ProtocolServer,
	ThreadRetentionScheduler,
} from "@artisan/backend";
import type { WorkspaceReplaceApprovalQueryResult } from "@artisan/protocol";
import type { ArtisanCommandReceipt } from "@artisan/transport";

import { Database } from "../../modules/backend/src/persistence/database";
import {
	OrchestrationCoordinators,
	OrchestrationRuns,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import {
	make_transport_test_harness,
	make_transport_test_harness_with_protocol_server,
} from "./message-channel-harness";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const receipt_authentication_key = Redacted.make(new Uint8Array(32).fill(13));
const now = "2026-07-12T16:00:00.000Z";
const temporary_directories: Array<string> = [];

interface NativeReplacementOptions {
	readonly expected: Uint8Array;
	readonly maximumBytes: number;
	readonly operationId: string;
	readonly path: string;
	readonly replacement: Uint8Array;
}

interface NativeController {
	readonly finalization_attempts: { value: number };
	readonly load_native_module: NonNullable<
		NativeBoundedRegularFileStoreOptions["load_native_module"]
	>;
	readonly replace_attempts: { value: number };
}

interface WorkspaceReplaceApprovalClient {
	readonly GetWorkspaceReplaceApproval: (input: {
		readonly approval_id: string;
		readonly thread_id: string;
	}) => Effect.Effect<WorkspaceReplaceApprovalQueryResult>;
	readonly RespondWorkspaceReplaceApproval: (input: {
		readonly approval_id: string;
		readonly approved: boolean;
		readonly command_id?: string;
		readonly thread_id: string;
	}) => Effect.Effect<ArtisanCommandReceipt>;
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
	const finalization_attempts = { value: 0 };
	const replace_attempts = { value: 0 };
	const receipts = new Map<string, NativeReplacementOptions>();

	class FakeNativeBoundedRegularFileStore {
		constructor(
			readonly root: string,
			_receipt_authentication_key: Uint8Array,
		) {}

		authorizeRoot(candidate_root: string) {
			return Promise.resolve(candidate_root === this.root);
		}

		close() {}

		async finalizeRegularFileReplacement(options: NativeReplacementOptions) {
			finalization_attempts.value += 1;

			const receipt = receipts.get(options.operationId);

			if (receipt === undefined || !replacement_options_match(receipt, options)) {
				throw new Error("replacement receipt intent changed");
			}

			receipts.delete(options.operationId);
		}

		async readRegularFile(path: string, maximum_bytes: number) {
			const bytes = new Uint8Array(await readFile(join(this.root, path)));

			if (bytes.byteLength > maximum_bytes) {
				throw new Error("file exceeds maximum");
			}

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

			const target = join(this.root, options.path);
			const current = new Uint8Array(await readFile(target));
			const matches = bytes_match(current, options.expected);

			if (!matches || options.replacement.byteLength > options.maximumBytes) {
				return "Changed";
			}

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
		finalization_attempts,
		load_native_module: () => ({
			NativeBoundedRegularFileStore: FakeNativeBoundedRegularFileStore,
			getNativeBuildDescriptor: () => ({
				architecture: "x86_64",
				operatingSystem: "windows",
				target: "x86_64-pc-windows-msvc",
				testHooksEnabled: false,
			}),
		}),
		replace_attempts,
	};
}

async function make_workspace() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-client-workspace-protocol-"));
	const root = join(directory, "workspace");

	temporary_directories.push(directory);

	await mkdir(join(root, "src"), { recursive: true });
	await writeFile(join(root, "src", "example.ts"), "before");

	return { database_path: join(directory, "artisan.db"), root };
}

function make_metadata_layer() {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "artisan_client_workspace_protocol_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.succeed(now),
	});
}

function make_inert_scheduler_layer() {
	return Layer.succeed(ThreadRetentionScheduler, {
		Schedule: () => Effect.never,
	});
}

function AtStep<A, E, R>(step: string, effect: Effect.Effect<A, E, R>) {
	return effect.pipe(
		Effect.mapError(
			(error) =>
				new Error(
					`${step}: ${error instanceof Error ? error.message : "unknown failure"}`,
					{ cause: error },
				),
		),
	);
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("ArtisanClient workspace operations with the backend ProtocolServer", () => {
	it("reads, replaces, reviews, and rolls back through real MessagePorts", async () => {
		const { database_path, root } = await make_workspace();
		const controller = make_native_controller();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			retention_scheduler: make_inert_scheduler_layer(),
			runtime_metadata: make_metadata_layer(),
			workspace_bounded_regular_file_store_registry:
				make_workspace_bounded_regular_file_store_registry_layer(
					[{ root, workspace_id: "workspace_public" }],
					{
						load_native_module: controller.load_native_module,
						receipt_authentication_key,
					},
				).pipe(Layer.provide(NodeFileSystem.layer)),
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const router = await runtime.runPromise(ProtocolRouter);
		const database = await runtime.runPromise(Database);

		await runtime.runPromise(
			router.Route({
				kind: "command",
				message_id: "create_workspace_public_thread",
				origin: "frontend",
				payload: {
					title: "Public workspace operations",
					type: "thread.create",
				},
				protocol_version: 1,
				schema_version: 1,
				sent_at: now,
				thread_id: "thread_workspace_public",
			}),
		);
		await runtime.runPromise(
			Effect.gen(function* () {
				yield* database.client.insert(OrchestrationCoordinators).values({
					active_run_id: "run_workspace_public",
					agent_id: "agent_workspace_public",
					created_at: now,
					display_name: "Coordinator",
					engine_id: "engine_workspace_public",
					role: "primary",
					thread_id: "thread_workspace_public",
					updated_at: now,
				});
				yield* database.client.insert(OrchestrationRuns).values({
					agent_id: "agent_workspace_public",
					created_at: now,
					engine_id: "engine_workspace_public",
					run_id: "run_workspace_public",
					status: "running",
					thread_id: "thread_workspace_public",
					updated_at: now,
					working_directory: root,
				});
			}),
		);

		const harness = await make_transport_test_harness_with_protocol_server(protocol_server, {
			client: { reconnect_delay_ms: 5 },
			drop_first_command_receipt: true,
		});

		try {
			const result = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const events_fiber = yield* harness.client.Events.pipe(
							Stream.filter(
								(event) => event.payload.type === "workspace.change.updated",
							),
							Stream.take(3),
							Stream.runCollect,
							Effect.forkScoped,
						);

						const initial = yield* harness.client.ReadWorkspaceFile({
							path: "src/example.ts",
							workspace_id: "workspace_public",
						});
						const replacement = yield* AtStep(
							"replace",
							harness.client.ReplaceWorkspaceFile({
								agent_id: "agent_workspace_public",
								approval_request: {
									reason: "Replace the workspace example with generated output.",
								},
								change_id: "change_workspace_public",
								command_id: "replace_workspace_public",
								content: "after",
								expected_before: initial.identity,
								path: "src/example.ts",
								run_id: "run_workspace_public",
								thread_id: "thread_workspace_public",
								workspace_id: "workspace_public",
							}),
						);
						const replaced = yield* harness.client.ReadWorkspaceFile({
							path: "src/example.ts",
							workspace_id: "workspace_public",
						});
						const before_review = yield* harness.client.ListWorkspaceChanges({
							thread_id: "thread_workspace_public",
							workspace_id: "workspace_public",
						});
						const reviewed = yield* AtStep(
							"review",
							harness.client.ReviewWorkspaceChange({
								change_id: "change_workspace_public",
								command_id: "review_workspace_public",
								thread_id: "thread_workspace_public",
							}),
						);

						yield* database.client
							.update(OrchestrationRuns)
							.set({ status: "complete", updated_at: now });

						const rolled_back = yield* AtStep(
							"rollback",
							harness.client.RollbackWorkspaceChange({
								change_id: "change_workspace_public",
								command_id: "rollback_workspace_public",
								expected_after: replaced.identity,
								thread_id: "thread_workspace_public",
							}),
						);
						const restored = yield* harness.client.ReadWorkspaceFile({
							path: "src/example.ts",
							workspace_id: "workspace_public",
						});
						const after_rollback = yield* harness.client.ListWorkspaceChanges({
							thread_id: "thread_workspace_public",
							workspace_id: "workspace_public",
						});
						const events = yield* Fiber.join(events_fiber);

						return {
							after_rollback,
							before_review,
							events: [...events],
							initial,
							replaced,
							replacement,
							restored,
							reviewed,
							rolled_back,
						};
					}),
				),
			);

			expect(result.initial.content).toBe("before");
			expect(result.replacement.status).toBe("duplicate");
			expect(result.replaced.content).toBe("after");
			expect(result.before_review.changes).toMatchObject([
				{
					change_id: "change_workspace_public",
					review_state: "needs_review",
					rollback_state: "available",
				},
			]);
			expect(result.reviewed.status).toBe("accepted");
			expect(result.rolled_back.status).toBe("accepted");
			expect(result.restored).toMatchObject({
				content: "before",
				identity: result.initial.identity,
			});
			expect(result.after_rollback.changes).toMatchObject([
				{
					change_id: "change_workspace_public",
					review_state: "rolled_back",
					rollback_state: "consumed",
				},
			]);
			expect(result.events.map((event) => event.payload)).toMatchObject([
				{ action: "recorded", type: "workspace.change.updated" },
				{ action: "reviewed", type: "workspace.change.updated" },
				{ action: "rolled_back", type: "workspace.change.updated" },
			]);
			expect(result.events.map((event) => event.journal_sequence)).toEqual(
				[...result.events]
					.map((event) => event.journal_sequence)
					.sort((left, right) => left - right),
			);
			expect(result.before_review.journal_sequence).toBeGreaterThanOrEqual(
				result.replacement.journal_sequence,
			);
			expect(harness.connector_snapshot()).toMatchObject({
				connections: 2,
				dropped_command_receipts: 1,
			});
			expect(controller.replace_attempts.value).toBe(2);
			expect(controller.finalization_attempts.value).toBe(2);
			expect(await readFile(join(root, "src", "example.ts"), "utf8")).toBe("before");
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});
});

describe("ArtisanClient workspace replacement approval routes", () => {
	it("queries a private approval diff and submits an approval response through MessagePorts", async () => {
		const harness = await make_transport_test_harness();
		const client = harness.client as typeof harness.client & WorkspaceReplaceApprovalClient;

		try {
			const result = await Effect.runPromise(
				client.GetWorkspaceReplaceApproval({
					approval_id: "approval_fixture",
					thread_id: "thread_fixture",
				}),
			);
			const receipt = await Effect.runPromise(
				client.RespondWorkspaceReplaceApproval({
					approval_id: "approval_fixture",
					approved: true,
					command_id: "approve_fixture",
					thread_id: "thread_fixture",
				}),
			);
			const snapshot = harness.protocol_snapshot();

			expect(result).toMatchObject({
				approval: {
					approval_id: "approval_fixture",
					state: "requested",
				},
				diff: {
					patch: "@@ -1,1 +1,1 @@\n-before\n+after\n",
				},
			});
			expect(receipt).toMatchObject({
				command_id: "approve_fixture",
				status: "accepted",
			});
			expect(snapshot.workspace_replace_approval_query_attempts).toMatchObject([
				{
					kind: "workspace.replace.approval.query",
					payload: { approval_id: "approval_fixture", thread_id: "thread_fixture" },
				},
			]);
			expect(snapshot.workspace_replace_approval_response_attempts).toMatchObject([
				{
					kind: "workspace.replace.approval.respond",
					message_id: "approve_fixture",
					payload: { approval_id: "approval_fixture", approved: true },
					thread_id: "thread_fixture",
				},
			]);
		} finally {
			await harness.dispose();
		}
	});
});
