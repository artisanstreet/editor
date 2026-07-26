import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Fiber, Layer, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	make_backend_runtime,
	ProtocolRouter,
	ProtocolServer,
	ThreadRetentionScheduler,
} from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";
import {
	AgentRuns,
	Assignments,
	OrchestrationCoordinators,
	OrchestrationGroups,
	OrchestrationRuns,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import { make_transport_test_harness_with_protocol_server } from "./message-channel-harness";
import { MakeNodeTestWorkspaceBoundedRegularFileStoreRegistryLayer } from "../backend/bounded-regular-file-store-harness";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const now = "2026-07-12T16:00:00.000Z";
const temporary_directories: Array<string> = [];

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
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			retention_scheduler: make_inert_scheduler_layer(),
			runtime_metadata: make_metadata_layer(),
			workspace_bounded_regular_file_store_registry:
				MakeNodeTestWorkspaceBoundedRegularFileStoreRegistryLayer([
					{ root, workspace_id: "workspace_public" },
				]),
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
				yield* database.client.insert(OrchestrationGroups).values({
					coordinator_agent_id: "agent_workspace_public",
					created_at: now,
					group_id: "group_workspace_review",
					journal_sequence: 1,
					max_concurrency: 2,
					state: "active",
					thread_id: "thread_workspace_public",
					updated_at: now,
					version: 1,
				});
				yield* database.client.insert(Assignments).values({
					active_run_id: "run_workspace_reviewer",
					agent_id: "agent_workspace_reviewer",
					assignment_id: "assignment_workspace_review",
					created_at: now,
					current_attempt: 1,
					engine_id: "engine_workspace_public",
					expected_result: "Review the workspace change",
					group_id: "group_workspace_review",
					instructions: "Review the workspace change",
					max_attempts: 1,
					parent_node_id: "node_workspace_review",
					permission_policy_json: "{}",
					profile: "reviewer",
					role: "reviewer",
					scope_json: "{}",
					state: "active",
					summary_contract: "Return a review outcome",
					updated_at: now,
					workspace_json: "{}",
				});
				yield* database.client.insert(AgentRuns).values({
					agent_id: "agent_workspace_reviewer",
					assignment_id: "assignment_workspace_review",
					attempt: 1,
					created_at: now,
					dispatch_status: "active",
					engine_id: "engine_workspace_public",
					group_id: "group_workspace_review",
					last_observation_sequence: 0,
					profile: "reviewer",
					run_id: "run_workspace_reviewer",
					state: "running",
					updated_at: now,
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
								assignment_id: "assignment_workspace_review",
								change_id: "change_workspace_public",
								comment: "The replacement needs a follow-up.",
								command_id: "review_workspace_public",
								group_id: "group_workspace_review",
								outcome: "changes_requested",
								raw_origin: {
									provider: "codex",
									reference: "workspace-review-turn",
								},
								reviewer_agent_id: "agent_workspace_reviewer",
								reviewer_kind: "graph",
								reviewer_run_id: "run_workspace_reviewer",
								thread_id: "thread_workspace_public",
							}),
						);
						const after_review = yield* harness.client.ListWorkspaceChanges({
							thread_id: "thread_workspace_public",
							workspace_id: "workspace_public",
						});

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
							after_review,
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
			expect(result.after_review.changes).toMatchObject([
				{
					review: {
						assignment_id: "assignment_workspace_review",
						comment: "The replacement needs a follow-up.",
						group_id: "group_workspace_review",
						outcome: "changes_requested",
						raw_origin: {
							provider: "codex",
							reference: "workspace-review-turn",
						},
						reviewer_agent_id: "agent_workspace_reviewer",
						reviewer_kind: "graph",
						reviewer_run_id: "run_workspace_reviewer",
					},
					review_state: "reviewed",
				},
			]);
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
			expect(await readFile(join(root, "src", "example.ts"), "utf8")).toBe("before");
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});
});
