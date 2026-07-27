import { promises as fs } from "node:fs";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { Deferred, Effect, Fiber, Layer, Option, Ref, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { EventEnvelope, ProjectRef } from "@artisan/protocol";
import {
	make_backend_runtime as make_unconfigured_backend_runtime,
	AgentGraphOrchestrator,
	make_codex_auto_compaction_mapping,
	make_thread_metadata_refiner_test_layer,
	make_codex_model_behaviour_provider,
	make_guidance_provider_registry_layer,
	make_inactive_model_behaviour_provider,
	make_model_behaviour_config_files_layer,
	make_model_behaviour_provider_registry_layer,
	make_unsupported_auto_compaction_mapping,
	ModelBehaviourConfigFiles,
	ModelBehaviourService,
	ProjectLocator,
	ProtocolServer,
	ThreadErasure,
	ThreadMetadataRefinementCoordinator,
	ThreadProjectAffinityCoordinator,
	SurfaceService,
	ThreadRetentionClock,
	WorkspaceEvidenceRecorder,
	WorkspaceChangeRepository,
} from "@artisan/backend";
import type {
	ArtisanClientError,
	ProjectCatalogUpdate,
	SurfaceUsageAggregateUpdate,
	ThreadListUpdate,
	WorkspaceConflictListUpdate,
} from "@artisan/transport";
import { Database } from "../../modules/backend/src/persistence/database";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import {
	GlobalGuidanceCanonical,
	GlobalGuidanceProviderSync,
	JournalCommands,
	JournalEvents,
	MessageImageAttachments,
	ModelBehaviourProviderStates,
	ModelBehaviourSettings,
	ThreadErasureClaims,
	ThreadProjectAffinityEvidence,
	AgentRuns,
	OrchestrationGroups,
	OrchestrationCoordinators,
	SurfaceUsageTotals,
	SurfaceItems,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

import {
	make_transport_test_harness_with_protocol_server,
	wait_for,
} from "./message-channel-harness";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));

const fixture_retention_now = "2026-07-18T20:00:00.000Z";

function make_backend_runtime(options: Parameters<typeof make_unconfigured_backend_runtime>[0]) {
	return make_unconfigured_backend_runtime({
		...options,
		retention_clock:
			options.retention_clock ??
			Layer.succeed(ThreadRetentionClock, {
				Now: Effect.succeed(fixture_retention_now),
			}),
		runtime_metadata:
			options.runtime_metadata ?? make_metadata_layer({ value: fixture_retention_now }),
	});
}

const temporary_directories: Array<string> = [];

const ProjectAlpha: ProjectRef = {
	display_name: "Alpha",
	project_id: "project_alpha",
	root_path: "C:/work/alpha",
};

const ProjectBeta: ProjectRef = {
	display_name: "Beta",
	project_id: "project_beta",
	root_path: "C:/work/beta",
};

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-client-protocol-server-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_metadata_layer(now: { value: string }) {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "artisan_client_protocol_server_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.sync(() => now.value),
	});
}

function make_project_locator_layer() {
	return Layer.succeed(ProjectLocator, {
		Locate: (location) => {
			const project = location.includes("beta")
				? ProjectBeta
				: location.includes("alpha")
					? ProjectAlpha
					: undefined;

			return Effect.succeed(
				project === undefined
					? Option.none()
					: Option.some({ project, source: "git_root" as const }),
			);
		},
	});
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("ArtisanClient with the backend ProtocolServer", () => {
	it("rejects legacy client-selected thread identities at the protocol boundary", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const protocol_server = await runtime.runPromise(ProtocolServer);

		try {
			const error = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* protocol_server.Open;
						yield* connection.Receive({
							kind: "hello",
							message_id: "legacy-create-hello",
							origin: "frontend",
							payload: {
								event_cursors: [],
								last_journal_sequence: 0,
								supported_protocol_versions: [1],
							},
							schema_version: 1,
							sent_at: fixture_retention_now,
						});
						yield* connection.Outbound.pipe(
							Stream.takeUntil((envelope) => envelope.kind === "replay.complete"),
							Stream.runDrain,
						);
						yield* connection.Receive({
							kind: "command",
							message_id: "legacy-create-command",
							origin: "frontend",
							payload: { title: "Client-selected identity", type: "thread.create" },
							protocol_version: 1,
							schema_version: 1,
							sent_at: fixture_retention_now,
							thread_id: "thread_client_selected",
						});

						return yield* connection.Outbound.pipe(Stream.take(1), Stream.runHead);
					}),
				),
			);

			expect(error).toMatchObject({
				_tag: "Some",
				value: {
					correlation_id: "legacy-create-command",
					kind: "protocol.error",
					payload: {
						code: "protocol.legacy_thread_create",
						retryable: false,
					},
				},
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("creates Forge-owned thread identities and exposes the authoritative project catalog", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
		try {
			const created = await Effect.runPromise(
				harness.client.CreateThread({ title: "Forge-owned identity" }),
			);
			const projects = await Effect.runPromise(harness.client.ListProjects);
			const runtime_catalog = await Effect.runPromise(harness.client.GetRuntimeCatalog);
			const project_updates = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const updates = yield* harness.client.SubscribeProjects;
						return yield* updates.pipe(
							Stream.take(1),
							Stream.runCollect,
							Effect.map(
								(items) => [...items] as ReadonlyArray<ProjectCatalogUpdate>,
							),
						);
					}),
				),
			);
			const detached = await Effect.runPromise(
				harness.client.DetachProject("project_not_attached"),
			);
			const threads = await Effect.runPromise(harness.client.ListThreads);

			expect(created.thread_id).toMatch(/^thread_\d+$/);
			expect(created.title).toBe("Forge-owned identity");
			expect(threads).toContainEqual(created);
			expect(projects).toEqual({ projects: [] });
			expect(runtime_catalog).toMatchObject({
				manifest: { harnesses: [], models: [], providers: [] },
			});
			expect(runtime_catalog).not.toHaveProperty("default_model_id");
			expect(project_updates).toEqual([{ snapshot: { projects: [] }, type: "snapshot" }]);
			expect(detached).toEqual({ projects: [] });
			await expect(
				Effect.runPromise(
					harness.client.CreateThread({
						project_id: "project_not_attached",
						title: "Invalid project",
					}),
				),
			).rejects.toThrow();
			expect(await Effect.runPromise(harness.client.ListThreads)).toHaveLength(1);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("closes the usage subscription snapshot boundary without missing a committed total", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const surfaces = await runtime.runPromise(SurfaceService);
		const snapshot_read = await Effect.runPromise(Deferred.make<void>());
		const release_snapshot = await Effect.runPromise(Deferred.make<void>());
		const original_snapshot = surfaces.AggregateUsageSnapshot;
		Object.assign(surfaces, {
			AggregateUsageSnapshot: (input: Parameters<typeof original_snapshot>[0]) =>
				original_snapshot(input).pipe(
					Effect.flatMap((snapshot) =>
						Deferred.succeed(snapshot_read, undefined).pipe(
							Effect.andThen(Deferred.await(release_snapshot)),
							Effect.as(snapshot),
						),
					),
				),
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const database = await runtime.runPromise(Database);
		const journal = await runtime.runPromise(JournalStore);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
		try {
			const created_thread_usage_boundary = await Effect.runPromise(
				harness.client.CreateThread({ title: "Usage boundary" }),
			);
			await Effect.runPromise(
				database.client.insert(SurfaceUsageTotals).values({
					group_id: "group_usage_boundary",
					input_tokens: 1,
					output_tokens: 1,
					run_id: "run_usage_boundary",
					updated_at: "2026-07-18T20:00:00.000Z",
				}),
			);
			const updates_promise = Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* harness.client.SubscribeSurfaceUsageAggregate({
							scope: "run",
							scope_id: "run_usage_boundary",
						});
						return yield* stream.pipe(
							Stream.take(2),
							Stream.runCollect,
							Effect.map(Array.from),
						);
					}),
				),
			);
			await Effect.runPromise(Deferred.await(snapshot_read));
			await Effect.runPromise(
				database.client
					.insert(SurfaceUsageTotals)
					.values({
						group_id: "group_usage_boundary",
						input_tokens: 5,
						output_tokens: 3,
						run_id: "run_usage_boundary",
						updated_at: "2026-07-18T20:00:01.000Z",
					})
					.onConflictDoUpdate({
						set: {
							input_tokens: 5,
							output_tokens: 3,
							updated_at: "2026-07-18T20:00:01.000Z",
						},
						target: SurfaceUsageTotals.run_id,
					}),
			);
			await Effect.runPromise(
				journal.AppendEvent({
					causation_id: "usage_boundary_event",
					correlation_id: "usage_boundary_event",
					payload: {
						type: "assistant.message_completed",
						message_id: "usage_boundary_event",
						text: "Visible",
					},
					run_id: "run_usage_boundary",
					thread_id: created_thread_usage_boundary.thread_id,
				}),
			);
			await Effect.runPromise(Deferred.succeed(release_snapshot, undefined));
			const updates = [...(await updates_promise)];
			expect(updates).toHaveLength(2);
			expect(updates[0]).toMatchObject({ snapshot: { aggregate: { input_tokens: 1 } } });
			expect(updates[1]).toMatchObject({
				snapshot: { aggregate: { input_tokens: 5, output_tokens: 3 } },
			});
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("closes the surface subscription snapshot boundary without missing a committed item", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const surfaces = await runtime.runPromise(SurfaceService);
		const snapshot_read = await Effect.runPromise(Deferred.make<void>());
		const release_snapshot = await Effect.runPromise(Deferred.make<void>());
		const original_snapshot = surfaces.ListSnapshot;
		Object.assign(surfaces, {
			ListSnapshot: (input: Parameters<typeof original_snapshot>[0]) =>
				original_snapshot(input).pipe(
					Effect.flatMap((snapshot) =>
						Deferred.succeed(snapshot_read, undefined).pipe(
							Effect.andThen(Deferred.await(release_snapshot)),
							Effect.as(snapshot),
						),
					),
				),
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const database = await runtime.runPromise(Database);
		const journal = await runtime.runPromise(JournalStore);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
		try {
			const created_thread_surface_boundary = await Effect.runPromise(
				harness.client.CreateThread({ title: "Surface boundary" }),
			);
			const updates_promise = Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* harness.client.SubscribeSurfaceItems({
							thread_id: created_thread_surface_boundary.thread_id,
						});
						return yield* stream.pipe(
							Stream.take(2),
							Stream.runCollect,
							Effect.map(Array.from),
						);
					}),
				),
			);
			await Effect.runPromise(Deferred.await(snapshot_read));
			await Effect.runPromise(
				database.client.insert(SurfaceItems).values({
					category: "work",
					kind: "message",
					observation_id: "surface_boundary_observation",
					occurred_at: "2026-07-18T20:00:00.000Z",
					run_id: "run_surface_boundary",
					sequence: 1,
					summary_json: JSON.stringify({ label: "Boundary message" }),
					surface_id: "surface_boundary_item",
					thread_id: created_thread_surface_boundary.thread_id,
				}),
			);
			await Effect.runPromise(
				journal.AppendEvent({
					causation_id: "surface_boundary_event",
					correlation_id: "surface_boundary_event",
					payload: {
						type: "assistant.message_completed",
						message_id: "surface_boundary_event",
						text: "Visible",
					},
					run_id: "run_surface_boundary",
					thread_id: created_thread_surface_boundary.thread_id,
				}),
			);
			await Effect.runPromise(Deferred.succeed(release_snapshot, undefined));
			const updates = [...(await updates_promise)];
			expect(updates).toHaveLength(2);
			expect(updates[0]).toMatchObject({ snapshot: { items: [] } });
			expect(updates[1]).toMatchObject({
				snapshot: { items: [{ surface_id: "surface_boundary_item" }] },
			});
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("keeps usage subscriptions scoped without advancing on unrelated runs", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const database = await runtime.runPromise(Database);
		const journal = await runtime.runPromise(JournalStore);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
		try {
			const created_thread_usage_scope = await Effect.runPromise(
				harness.client.CreateThread({ title: "Usage scope" }),
			);
			await Effect.runPromise(
				database.client.insert(SurfaceUsageTotals).values([
					{
						group_id: "group_target",
						input_tokens: 2,
						output_tokens: 1,
						run_id: "run_target",
						updated_at: "2026-07-18T20:00:00.000Z",
					},
					{
						group_id: "group_other",
						input_tokens: 9,
						output_tokens: 9,
						run_id: "run_other",
						updated_at: "2026-07-18T20:00:00.000Z",
					},
				]),
			);
			const subscribed = await Effect.runPromise(Deferred.make<void>());
			const updates_promise = Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream: Stream.Stream<
							SurfaceUsageAggregateUpdate,
							ArtisanClientError
						> = yield* harness.client.SubscribeSurfaceUsageAggregate({
							scope: "group",
							scope_id: "group_target",
						});
						yield* Deferred.succeed(subscribed, undefined);
						return yield* stream.pipe(
							Stream.take(2),
							Stream.runCollect,
							Effect.map(
								(updates): ReadonlyArray<SurfaceUsageAggregateUpdate> =>
									Array.from(updates),
							),
						);
					}),
				),
			);
			await Effect.runPromise(Deferred.await(subscribed));
			const unrelated = await Effect.runPromise(
				journal.AppendEvent({
					causation_id: "usage_other",
					correlation_id: "usage_other",
					payload: {
						type: "assistant.message_completed",
						message_id: "usage_other",
						text: "Other",
					},
					run_id: "run_other",
					thread_id: created_thread_usage_scope.thread_id,
				}),
			);
			const target = await Effect.runPromise(
				journal.AppendEvent({
					causation_id: "usage_target",
					correlation_id: "usage_target",
					payload: {
						type: "assistant.message_completed",
						message_id: "usage_target",
						text: "Target",
					},
					run_id: "run_target",
					thread_id: created_thread_usage_scope.thread_id,
				}),
			);
			const updates = [...(await updates_promise)];
			expect(updates).toHaveLength(2);
			expect(updates[1]).toMatchObject({
				snapshot: {
					aggregate: { input_tokens: 2, output_tokens: 1, scope_id: "group_target" },
					journal_sequence: target.journal_sequence,
				},
			});
			expect(updates[1]!.snapshot.journal_sequence).toBeGreaterThan(
				unrelated.journal_sequence,
			);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("replaces an active run usage subscription with unknown totals after thread erasure", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const database = await runtime.runPromise(Database);
		const erasure = await runtime.runPromise(ThreadErasure);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
		try {
			const created_thread_usage_erasure = await Effect.runPromise(
				harness.client.CreateThread({ title: "Usage erasure" }),
			);
			await Effect.runPromise(
				database.client.insert(OrchestrationGroups).values({
					coordinator_agent_id: "agent_usage_erasure",
					created_at: "2026-07-18T20:00:00.000Z",
					group_id: "group_usage_erasure",
					journal_sequence: 1,
					max_concurrency: 1,
					state: "complete",
					thread_id: created_thread_usage_erasure.thread_id,
					updated_at: "2026-07-18T20:00:00.000Z",
					version: 1,
				}),
			);
			await Effect.runPromise(
				database.client.insert(AgentRuns).values({
					agent_id: "agent_usage_erasure",
					assignment_id: "assignment_usage_erasure",
					attempt: 1,
					created_at: "2026-07-18T20:00:00.000Z",
					dispatch_status: "completed",
					engine_id: "fake",
					group_id: "group_usage_erasure",
					last_observation_sequence: 1,
					profile: "default",
					run_id: "run_usage_erasure",
					state: "complete",
					updated_at: "2026-07-18T20:00:00.000Z",
				}),
			);
			await Effect.runPromise(
				database.client.insert(SurfaceUsageTotals).values({
					group_id: "group_usage_erasure",
					input_tokens: 13,
					output_tokens: 8,
					run_id: "run_usage_erasure",
					updated_at: "2026-07-18T20:00:00.000Z",
				}),
			);

			const initial_delivered = await Effect.runPromise(Deferred.make<void>());
			const updates_promise: Promise<ReadonlyArray<SurfaceUsageAggregateUpdate>> =
				Effect.runPromise(
					Effect.scoped(
						Effect.gen(function* () {
							const stream = yield* harness.client.SubscribeSurfaceUsageAggregate({
								scope: "run",
								scope_id: "run_usage_erasure",
							});
							return yield* stream.pipe(
								Stream.tap((update) =>
									update.snapshot.aggregate.input_tokens === 13
										? Deferred.succeed(initial_delivered, undefined).pipe(
												Effect.asVoid,
											)
										: Effect.void,
								),
								Stream.take(2),
								Stream.runCollect,
								Effect.map(
									(updates): ReadonlyArray<SurfaceUsageAggregateUpdate> =>
										Array.from(updates),
								),
							);
						}),
					),
				);
			await Effect.runPromise(Deferred.await(initial_delivered));
			await Effect.runPromise(
				database.client.insert(ThreadErasureClaims).values({
					claimed_at: "2026-07-18T20:01:00.000Z",
					thread_id: created_thread_usage_erasure.thread_id,
				}),
			);
			await Effect.runPromise(erasure.ResumeClaimed("2026-07-18T20:01:00.000Z"));

			const updates = [...(await updates_promise)];
			expect(updates).toHaveLength(2);
			expect(updates[0]).toMatchObject({
				snapshot: {
					aggregate: {
						input_tokens: 13,
						output_tokens: 8,
						scope: "run",
						scope_id: "run_usage_erasure",
					},
				},
			});
			expect(updates[1]).toMatchObject({
				snapshot: {
					aggregate: { scope: "run", scope_id: "run_usage_erasure" },
				},
			});
			expect(updates[1]!.snapshot.aggregate).not.toHaveProperty("input_tokens");
			expect(updates[1]!.snapshot.aggregate).not.toHaveProperty("output_tokens");
			expect(
				await Effect.runPromise(database.client.select().from(SurfaceUsageTotals)),
			).toEqual([]);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("lists, subscribes, reconnects, and erases attributed workspace conflicts", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const erasure = await runtime.runPromise(ThreadErasure);
		const conflicts = await runtime.runPromise(WorkspaceChangeRepository);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server, {
			client: { reconnect_delay_ms: 5 },
		});
		try {
			const created_thread_conflict_projection = await Effect.runPromise(
				harness.client.CreateThread({ title: "Conflict projection" }),
			);
			const subscribed = await Effect.runPromise(Deferred.make<void>());
			const reconnect_snapshot = await Effect.runPromise(Deferred.make<void>());
			let awaiting_reconnect_snapshot = false;
			const updates_promise: Promise<ReadonlyArray<WorkspaceConflictListUpdate>> =
				Effect.runPromise(
					Effect.scoped(
						Effect.gen(function* () {
							const stream: Stream.Stream<
								WorkspaceConflictListUpdate,
								import("@artisan/transport").ArtisanClientError
							> = yield* harness.client.SubscribeWorkspaceConflicts(
								created_thread_conflict_projection.thread_id,
							);
							yield* Deferred.succeed(subscribed, undefined);
							return yield* stream.pipe(
								Stream.tap((update) =>
									awaiting_reconnect_snapshot && update.type === "snapshot"
										? Deferred.succeed(reconnect_snapshot, undefined).pipe(
												Effect.asVoid,
											)
										: Effect.void,
								),
								Stream.drop(1),
								Stream.takeUntil(
									(update) => update.snapshot.conflicts.length === 0,
								),
								Stream.runCollect,
								Effect.map(
									(updates): ReadonlyArray<WorkspaceConflictListUpdate> =>
										Array.from(updates),
								),
							);
						}),
					),
				);
			await Effect.runPromise(Deferred.await(subscribed));
			expect(
				await runtime.runPromise(
					conflicts.ListConflicts(created_thread_conflict_projection.thread_id),
				),
			).toEqual([]);
			const expected_before = {
				algorithm: "sha256" as const,
				byte_count: 1,
				content_hash: "a".repeat(64),
			};
			const observed = {
				algorithm: "sha256" as const,
				byte_count: 2,
				content_hash: "b".repeat(64),
			};
			await runtime.runPromise(
				conflicts.ClaimReplace({
					_tag: "replace",
					agent_id: "agent_conflict",
					change_id: "change_conflict",
					expected_before,
					intended_after: {
						algorithm: "sha256",
						byte_count: 3,
						content_hash: "c".repeat(64),
					},
					message_id: "source_conflict",
					path: "src/file.ts",
					raw_origin: { provider: "codex", reference: "conflict-origin" },
					request_fingerprint: "d".repeat(64),
					run_id: "run_conflict",
					sent_at: "2026-07-18T20:00:00.000Z",
					thread_id: created_thread_conflict_projection.thread_id,
					workspace_id: "workspace_conflict",
				}),
			);
			await runtime.runPromise(
				conflicts.ReconcileChanged({
					message_id: "source_conflict",
					observation: "native_changed",
					observed_identity: observed,
				}),
			);
			await new Promise((resolve) => setTimeout(resolve, 20));
			expect(
				await runtime.runPromise(
					conflicts.ListConflicts(created_thread_conflict_projection.thread_id),
				),
			).toHaveLength(1);
			const connections = harness.connector_snapshot().connections;
			awaiting_reconnect_snapshot = true;
			harness.close_current_connection();
			const after_reconnect = await Effect.runPromise(
				harness.client.ListWorkspaceConflicts(created_thread_conflict_projection.thread_id),
			);
			await wait_for(() => harness.connector_snapshot().connections > connections);
			await Effect.runPromise(Deferred.await(reconnect_snapshot));
			expect(after_reconnect.conflicts).toHaveLength(1);
			await Effect.runPromise(
				harness.client.Command({
					command_id: "conflict_projection_archive",
					payload: { type: "thread.archive" },
					thread_id: created_thread_conflict_projection.thread_id,
				}),
			);
			await Effect.runPromise(
				erasure.CleanupExpired("2026-07-19T00:00:00.000Z", "2026-07-19T00:00:00.000Z"),
			);
			const after_erasure = await Effect.runPromise(
				harness.client.ListWorkspaceConflicts(created_thread_conflict_projection.thread_id),
			);
			const updates = await updates_promise;
			expect(updates[0]?.snapshot.conflicts).toHaveLength(1);
			expect(updates.at(-1)?.snapshot.conflicts).toEqual([]);
			expect(after_erasure.conflicts).toEqual([]);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("projects session and canonical surfaces through real MessagePorts and reconnects", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const database = await runtime.runPromise(Database);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server, {
			client: { reconnect_delay_ms: 5 },
		});

		try {
			const created_thread_surface_session = await Effect.runPromise(
				harness.client.CreateThread({ title: "Surface session" }),
			);
			await Effect.runPromise(
				database.client.insert(OrchestrationCoordinators).values({
					active_run_id: null,
					agent_id: "agent_surface_session",
					created_at: "2026-07-18T10:00:00.000Z",
					display_name: "Session coordinator",
					engine_id: "fake",
					role: "coordinator",
					thread_id: created_thread_surface_session.thread_id,
					updated_at: "2026-07-18T10:00:00.000Z",
				}),
			);

			const initial = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const session_stream = yield* harness.client.SubscribeThreadSession(
							created_thread_surface_session.thread_id,
						);
						const surface_stream = yield* harness.client.SubscribeSurfaceItems({
							thread_id: created_thread_surface_session.thread_id,
						});
						const usage_stream = yield* harness.client.SubscribeSurfaceUsageAggregate({
							scope: "run",
							scope_id: "run_surface_session",
						});

						return {
							session: yield* harness.client.GetThreadSession(
								created_thread_surface_session.thread_id,
							),
							session_update: yield* session_stream.pipe(
								Stream.take(1),
								Stream.runHead,
							),
							surfaces: yield* harness.client.ListSurfaceItems({
								thread_id: created_thread_surface_session.thread_id,
							}),
							surface_update: yield* surface_stream.pipe(
								Stream.take(1),
								Stream.runHead,
							),
							usage: yield* harness.client.GetSurfaceUsageAggregate({
								scope: "run",
								scope_id: "run_surface_session",
							}),
							usage_update: yield* usage_stream.pipe(Stream.take(1), Stream.runHead),
						};
					}),
				),
			);

			expect(initial).toMatchObject({
				session: {
					auto_steer_enabled: true,
					thread_id: created_thread_surface_session.thread_id,
				},
				session_update: {
					_tag: "Some",
					value: {
						snapshot: { thread_id: created_thread_surface_session.thread_id },
						type: "snapshot",
					},
				},
				surfaces: { items: [] },
				surface_update: {
					_tag: "Some",
					value: { snapshot: { items: [] }, type: "snapshot" },
				},
				usage: { aggregate: { scope: "run", scope_id: "run_surface_session" } },
				usage_update: {
					_tag: "Some",
					value: {
						snapshot: { aggregate: { scope: "run", scope_id: "run_surface_session" } },
						type: "snapshot",
					},
				},
			});

			const live_session_updates = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* harness.client.SubscribeThreadSession(
							created_thread_surface_session.thread_id,
						);
						const fiber = yield* stream.pipe(
							Stream.take(2),
							Stream.runCollect,
							Effect.forkScoped,
						);
						yield* harness.client.Command({
							command_id: "surface_session_disable_steering",
							payload: { enabled: false, type: "thread.auto_steer.update" },
							thread_id: created_thread_surface_session.thread_id,
						});
						return [...(yield* Fiber.join(fiber))];
					}),
				),
			);
			expect(live_session_updates.at(-1)).toMatchObject({
				snapshot: {
					auto_steer_enabled: false,
					thread_id: created_thread_surface_session.thread_id,
				},
				type: "snapshot",
			});

			const connections = harness.connector_snapshot().connections;
			harness.close_current_connection();
			const reconnected = await Effect.runPromise(
				harness.client.GetThreadSession(created_thread_surface_session.thread_id),
			);
			await wait_for(() => harness.connector_snapshot().connections > connections);
			expect(reconnected).toMatchObject({
				auto_steer_enabled: false,
				thread_id: created_thread_surface_session.thread_id,
			});
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("delivers a group transition committed at the subscription snapshot boundary", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const graph = await runtime.runPromise(AgentGraphOrchestrator);
		const snapshot_read = await Effect.runPromise(Deferred.make<void>());
		const release_snapshot = await Effect.runPromise(Deferred.make<void>());
		const original_snapshot = graph.ListGroupsSnapshot;
		Object.assign(graph, {
			ListGroupsSnapshot: (thread_id: string, include_terminal: boolean) =>
				original_snapshot(thread_id, include_terminal).pipe(
					Effect.flatMap((snapshot) =>
						Deferred.succeed(snapshot_read, undefined).pipe(
							Effect.andThen(Deferred.await(release_snapshot)),
							Effect.as(snapshot),
						),
					),
				),
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const database = await runtime.runPromise(Database);
		const journal = await runtime.runPromise(JournalStore);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
		try {
			const created_thread_group_boundary = await Effect.runPromise(
				harness.client.CreateThread({ title: "Group boundary" }),
			);
			await Effect.runPromise(
				database.client.insert(OrchestrationGroups).values({
					group_id: "group_boundary",
					thread_id: created_thread_group_boundary.thread_id,
					coordinator_agent_id: "agent_boundary",
					state: "running",
					max_concurrency: 1,
					version: 1,
					journal_sequence: 1,
					created_at: "2026-07-18T10:00:00.000Z",
					updated_at: "2026-07-18T10:00:00.000Z",
				}),
			);

			const updates_promise = Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* harness.client.SubscribeOrchestrationGroups(
							created_thread_group_boundary.thread_id,
							false,
						);

						return yield* stream.pipe(
							Stream.take(2),
							Stream.runCollect,
							Effect.map(Array.from),
						);
					}),
				),
			);
			await Effect.runPromise(Deferred.await(snapshot_read));
			await Effect.runPromise(
				database.client.update(OrchestrationGroups).set({
					state: "complete",
					updated_at: "2026-07-18T10:01:00.000Z",
					version: 2,
				}),
			);
			const transition = await Effect.runPromise(
				journal.AppendEvent({
					causation_id: "group_boundary_transition",
					correlation_id: "group_boundary_transition",
					payload: {
						type: "orchestration.graph.lifecycle",
						group_id: "group_boundary",
						node_id: "group_boundary",
						node_type: "orchestration_group",
						state: "complete",
						action: "completed_at_snapshot_boundary",
					},
					thread_id: created_thread_group_boundary.thread_id,
				}),
			);
			await Effect.runPromise(Deferred.succeed(release_snapshot, undefined));

			const updates = await updates_promise;
			expect(updates).toMatchObject([
				{
					type: "snapshot",
					snapshot: { groups: [{ group_id: "group_boundary", state: "running" }] },
				},
				{ type: "patch", snapshot: { groups: [] } },
			]);
			expect(updates[1]).toMatchObject({
				snapshot: { journal_sequence: transition.journal_sequence },
			});
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("does not append snapshot history again when subscription registration races a pending journal tail", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const journal = await runtime.runPromise(JournalStore);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
		try {
			const created_thread_snapshot_race = await Effect.runPromise(
				harness.client.CreateThread({ title: "Snapshot race" }),
			);
			await runtime.runPromise(
				journal.AppendEvent({
					causation_id: "snapshot_race_first",
					correlation_id: "snapshot_race_first",
					payload: {
						type: "assistant.message_completed",
						message_id: "snapshot_race_first",
						text: "Already represented by the snapshot.",
					},
					thread_id: created_thread_snapshot_race.thread_id,
				}),
			);

			const updates = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* harness.client.SubscribeThreadTranscript(
							created_thread_snapshot_race.thread_id,
						);
						const fiber = yield* stream.pipe(
							Stream.take(2),
							Stream.runCollect,
							Effect.forkScoped,
						);
						yield* journal.AppendEvent({
							causation_id: "snapshot_race_second",
							correlation_id: "snapshot_race_second",
							payload: {
								type: "assistant.message_completed",
								message_id: "snapshot_race_second",
								text: "Only this entry should append.",
							},
							thread_id: created_thread_snapshot_race.thread_id,
						});
						return [...(yield* Fiber.join(fiber))];
					}),
				),
			);

			expect(updates[0]).toMatchObject({
				type: "snapshot",
				transcript: {
					entries: [{ payload: { text: "Already represented by the snapshot." } }],
				},
			});
			expect(updates[1]).toMatchObject({
				type: "append",
				entries: [{ payload: { text: "Only this entry should append." } }],
			});
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("queries and streams durable canonical conversation patches through real MessagePorts", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const journal = await runtime.runPromise(JournalStore);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
		try {
			const created_thread_conversation = await Effect.runPromise(
				harness.client.CreateThread({ title: "Conversation" }),
			);
			const updates = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* harness.client.SubscribeConversation(
							created_thread_conversation.thread_id,
						);
						const fiber = yield* stream.pipe(
							Stream.take(2),
							Stream.runCollect,
							Effect.forkScoped,
						);
						yield* journal.AppendEvent({
							causation_id: "conversation_message",
							correlation_id: "conversation_message",
							payload: {
								message_id: "conversation_message",
								reason: "no_active_run",
								text: "Canonical user message.",
								type: "thread.message_queued",
								working_directory: "C:\\workspace",
							},
							thread_id: created_thread_conversation.thread_id,
						});
						return [...(yield* Fiber.join(fiber))];
					}),
				),
			);
			const queried = await Effect.runPromise(
				harness.client.GetConversation({
					thread_id: created_thread_conversation.thread_id,
				}),
			);
			expect(updates[0]).toMatchObject({
				snapshot: { items: [], last_patch_sequence: 0 },
				type: "snapshot",
			});
			expect(updates[1]).toMatchObject({
				batch: {
					from_sequence: 1,
					patches: [
						{ type: "turn_upsert" },
						{ item: { text: "Canonical user message.", type: "user_message" } },
					],
					to_sequence: 2,
				},
				type: "patch",
			});
			expect(queried).toMatchObject({
				items: [{ text: "Canonical user message.", type: "user_message" }],
				last_patch_sequence: 2,
			});
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("reads a thread-owned message image attachment through the typed client", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const database = await runtime.runPromise(Database);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
		const bytes = new Uint8Array([0x89, 0x50, 0x4e, 0x47]);
		try {
			await runtime.runPromise(
				database.client.transaction((transaction) =>
					Effect.gen(function* () {
						yield* transaction.insert(JournalCommands).values({
							accepted_at: fixture_retention_now,
							message_id: "message_image_query",
							origin: "frontend",
							payload_json: "{}",
							payload_type: "thread.send_message",
							schema_version: 1,
							sent_at: fixture_retention_now,
							status: "accepted",
							thread_id: "thread_image_query",
						});
						yield* transaction.insert(MessageImageAttachments).values({
							attachment_id: "attachment_image_query",
							content: Buffer.from(bytes),
							media_type: "image/png",
							message_id: "message_image_query",
							name: "query.png",
							position: 0,
							size_bytes: bytes.byteLength,
						});
					}),
				),
			);

			const found = await Effect.runPromise(
				harness.client.GetMessageImageAttachment({
					attachment_id: "attachment_image_query",
					thread_id: "thread_image_query",
				}),
			);
			const denied = await Effect.runPromise(
				harness.client.GetMessageImageAttachment({
					attachment_id: "attachment_image_query",
					thread_id: "thread_other",
				}),
			);

			expect(Option.getOrThrow(found)).toMatchObject({
				id: "attachment_image_query",
				media_type: "image/png",
				name: "query.png",
				size_bytes: bytes.byteLength,
			});
			expect([...Option.getOrThrow(found).bytes]).toEqual([...bytes]);
			expect(Option.isNone(denied)).toBe(true);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("reads and appends the journal-derived safe transcript through real MessagePorts", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const journal = await runtime.runPromise(JournalStore);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server, {
			client: { event_capacity: 1_000 },
		});
		try {
			const created_thread_transcript = await Effect.runPromise(
				harness.client.CreateThread({ title: "Transcript" }),
			);
			const updates = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* harness.client.SubscribeThreadTranscript(
							created_thread_transcript.thread_id,
						);
						const fiber = yield* stream.pipe(
							Stream.take(2),
							Stream.runCollect,
							Effect.forkScoped,
						);
						yield* journal.AppendEvent({
							causation_id: "transcript_append",
							correlation_id: "transcript_append",
							payload: {
								type: "assistant.message_completed",
								message_id: "assistant_transcript",
								text: "Safe transcript content.",
							},
							thread_id: created_thread_transcript.thread_id,
						});
						return [...(yield* Fiber.join(fiber))];
					}),
				),
			);
			const queried = await Effect.runPromise(
				harness.client.GetThreadTranscript({
					thread_id: created_thread_transcript.thread_id,
				}),
			);
			expect(updates).toMatchObject([
				{ type: "snapshot", transcript: { status: "available" } },
				{
					type: "append",
					entries: [
						{
							payload: {
								type: "assistant.message_completed",
								text: "Safe transcript content.",
							},
						},
					],
				},
			]);
			expect(queried).toMatchObject({
				status: "available",
				entries: [
					{
						journal_sequence: expect.any(Number),
						payload: { text: "Safe transcript content." },
					},
				],
			});
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("re-establishes a transcript projection after a real MessagePort reconnect", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const journal = await runtime.runPromise(JournalStore);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server, {
			client: { reconnect_delay_ms: 5 },
		});
		try {
			const created_thread_transcript_reconnect = await Effect.runPromise(
				harness.client.CreateThread({ title: "Reconnect transcript" }),
			);
			const updates = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const initial_snapshot = yield* Deferred.make<void>();
						const reconnect_snapshot = yield* Deferred.make<void>();
						let snapshot_count = 0;
						const stream = yield* harness.client.SubscribeThreadTranscript(
							created_thread_transcript_reconnect.thread_id,
						);
						const fiber = yield* stream.pipe(
							Stream.tap((update) => {
								if (update.type !== "snapshot") return Effect.void;
								snapshot_count += 1;
								return Deferred.succeed(
									snapshot_count === 1 ? initial_snapshot : reconnect_snapshot,
									undefined,
								).pipe(Effect.asVoid);
							}),
							Stream.take(3),
							Stream.runCollect,
							Effect.forkScoped,
						);
						yield* Deferred.await(initial_snapshot);
						harness.close_current_connection();
						yield* Effect.promise(() =>
							wait_for(() => harness.connector_snapshot().connections >= 2),
						);
						yield* Deferred.await(reconnect_snapshot);
						yield* journal.AppendEvent({
							causation_id: "reconnect_transcript_append",
							correlation_id: "reconnect_transcript_append",
							payload: {
								type: "assistant.message_completed",
								message_id: "assistant_reconnect",
								text: "Delivered after reconnect.",
							},
							thread_id: created_thread_transcript_reconnect.thread_id,
						});
						const all = [...(yield* Fiber.join(fiber))];
						return all;
					}),
				),
			);
			expect(updates.filter((update) => update.type === "snapshot")).not.toHaveLength(0);
			const append = updates.find((update) => update.type === "append");
			expect(append).toMatchObject({
				entries: [{ payload: { text: "Delivered after reconnect." } }],
			});
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	}, 30_000);

	it("replaces live transcript content with an explicit erased snapshot", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const journal = await runtime.runPromise(JournalStore);
		const erasure = await runtime.runPromise(ThreadErasure);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
		try {
			const created_thread_transcript_erased = await Effect.runPromise(
				harness.client.CreateThread({ title: "Erase transcript" }),
			);
			await Effect.runPromise(
				journal.AppendEvent({
					causation_id: "erase_transcript_message",
					correlation_id: "erase_transcript_message",
					payload: {
						type: "assistant.message_completed",
						message_id: "erase_assistant",
						text: "This must disappear.",
					},
					thread_id: created_thread_transcript_erased.thread_id,
				}),
			);
			const updates = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* harness.client.SubscribeThreadTranscript(
							created_thread_transcript_erased.thread_id,
						);
						const fiber = yield* stream.pipe(
							Stream.take(2),
							Stream.runCollect,
							Effect.forkScoped,
						);
						yield* harness.client.Command({
							command_id: "erase_transcript_archive",
							payload: { type: "thread.archive" },
							thread_id: created_thread_transcript_erased.thread_id,
						});
						yield* erasure.CleanupExpired(
							"2026-07-19T00:00:00.000Z",
							"2026-07-19T00:00:00.000Z",
						);
						return [...(yield* Fiber.join(fiber))];
					}),
				),
			);
			expect(updates).toMatchObject([
				{
					type: "snapshot",
					transcript: {
						status: "available",
						entries: [{ payload: { text: "This must disappear." } }],
					},
				},
				{ type: "snapshot", transcript: { status: "erased", entries: [] } },
			]);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("does not duplicate transcript appends when a later journal fact is visible during an earlier notification", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const journal = await runtime.runPromise(JournalStore);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
		try {
			const created_thread_transcript_race = await Effect.runPromise(
				harness.client.CreateThread({ title: "Race transcript" }),
			);
			const updates = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* harness.client.SubscribeThreadTranscript(
							created_thread_transcript_race.thread_id,
						);
						const fiber = yield* stream.pipe(
							Stream.take(3),
							Stream.runCollect,
							Effect.forkScoped,
						);
						yield* Effect.all(
							[
								journal.AppendEvent({
									causation_id: "race_one",
									correlation_id: "race_one",
									payload: {
										type: "assistant.message_completed",
										message_id: "race_message_one",
										text: "one",
									},
									thread_id: created_thread_transcript_race.thread_id,
								}),
								journal.AppendEvent({
									causation_id: "race_two",
									correlation_id: "race_two",
									payload: {
										type: "assistant.message_completed",
										message_id: "race_message_two",
										text: "two",
									},
									thread_id: created_thread_transcript_race.thread_id,
								}),
							],
							{ concurrency: 1, discard: true },
						);
						return [...(yield* Fiber.join(fiber))];
					}),
				),
			);
			const appended = updates
				.filter((update) => update.type === "append")
				.flatMap((update) => update.entries.map((entry) => entry.event_id));
			expect(appended).toHaveLength(2);
			expect(new Set(appended).size).toBe(2);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("returns latest safe transcript pages despite more than one limit of mixed journal facts", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const journal = await runtime.runPromise(JournalStore);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server, {
			client: { event_capacity: 1_000 },
		});
		try {
			const created_thread_history = await Effect.runPromise(
				harness.client.CreateThread({ title: "History" }),
			);
			await Effect.runPromise(
				Effect.forEach(
					Array.from({ length: 5 }, (_, index) => index + 1),
					(index) =>
						Effect.all(
							[
								journal.AppendEvent({
									causation_id: `history_safe_${index}`,
									correlation_id: `history_safe_${index}`,
									payload: {
										type: "assistant.message_completed",
										message_id: `history_message_${index}`,
										text: `safe ${index}`,
									},
									thread_id: created_thread_history.thread_id,
								}),
								journal.AppendEvent({
									causation_id: `history_raw_${index}`,
									correlation_id: `history_raw_${index}`,
									payload: {
										type: "filesystem.mutation",
										operation: "write",
										path: `C:/private/${index}`,
									},
									thread_id: created_thread_history.thread_id,
								}),
							],
							{ concurrency: 1, discard: true },
						),
				),
			);
			const newest = await Effect.runPromise(
				harness.client.GetThreadTranscript({
					thread_id: created_thread_history.thread_id,
					limit: 2,
				}),
			);
			const previous = await Effect.runPromise(
				harness.client.GetThreadTranscript({
					thread_id: created_thread_history.thread_id,
					limit: 2,
					before_journal_sequence: newest.next_before_journal_sequence,
				}),
			);
			expect(newest).toMatchObject({
				entries: [{ payload: { text: "safe 4" } }, { payload: { text: "safe 5" } }],
			});
			expect(newest.next_before_journal_sequence).toBeDefined();
			expect(previous).toMatchObject({
				entries: [{ payload: { text: "safe 2" } }, { payload: { text: "safe 3" } }],
			});
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	}, 30_000);

	it("discovers thread groups and delivers an ordered replacement patch through real MessagePorts", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const database = await runtime.runPromise(Database);
		const journal = await runtime.runPromise(JournalStore);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
		try {
			const created_thread_groups = await Effect.runPromise(
				harness.client.CreateThread({ title: "Groups" }),
			);
			const created_thread_groups_other = await Effect.runPromise(
				harness.client.CreateThread({ title: "Other Groups" }),
			);
			await Effect.runPromise(
				database.client.insert(OrchestrationGroups).values({
					group_id: "group_other",
					thread_id: created_thread_groups_other.thread_id,
					coordinator_agent_id: "agent_other",
					state: "running",
					max_concurrency: 1,
					version: 1,
					journal_sequence: 3,
					created_at: "2026-07-18T10:00:00.000Z",
					updated_at: "2026-07-18T10:00:00.000Z",
				}),
			);
			await Effect.runPromise(
				database.client.insert(OrchestrationGroups).values({
					group_id: "group_live",
					thread_id: created_thread_groups.thread_id,
					coordinator_agent_id: "agent_coordinator",
					state: "running",
					max_concurrency: 2,
					version: 1,
					journal_sequence: 2,
					created_at: "2026-07-18T10:00:00.000Z",
					updated_at: "2026-07-18T10:00:00.000Z",
				}),
			);
			await Effect.runPromise(
				database.client.insert(OrchestrationGroups).values({
					group_id: "group_terminal",
					thread_id: created_thread_groups.thread_id,
					coordinator_agent_id: "agent_coordinator",
					state: "complete",
					max_concurrency: 2,
					version: 1,
					journal_sequence: 2,
					created_at: "2026-07-18T10:00:00.000Z",
					updated_at: "2026-07-18T10:00:00.000Z",
				}),
			);
			const discovered = await Effect.runPromise(
				harness.client.ListOrchestrationGroups(created_thread_groups.thread_id, false),
			);
			const with_terminal = await Effect.runPromise(
				harness.client.ListOrchestrationGroups(created_thread_groups.thread_id, true),
			);
			const updates = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* harness.client.SubscribeOrchestrationGroups(
							created_thread_groups.thread_id,
							false,
						);
						const fiber = yield* stream.pipe(
							Stream.take(2),
							Stream.runCollect,
							Effect.forkScoped,
						);
						yield* journal.AppendEvent({
							causation_id: "group_other_patch",
							correlation_id: "group_other_patch",
							payload: {
								type: "orchestration.graph.lifecycle",
								group_id: "group_other",
								node_id: "group_other",
								node_type: "orchestration_group",
								state: "running",
								action: "unrelated",
							},
							thread_id: created_thread_groups_other.thread_id,
						});
						const own = yield* journal.AppendEvent({
							causation_id: "group_patch",
							correlation_id: "group_patch",
							payload: {
								type: "orchestration.graph.lifecycle",
								group_id: "group_live",
								node_id: "group_live",
								node_type: "orchestration_group",
								state: "running",
								action: "refreshed",
							},
							thread_id: created_thread_groups.thread_id,
						});
						return { own, updates: [...(yield* Fiber.join(fiber))] };
					}),
				),
			);
			expect(discovered).toMatchObject({
				groups: [{ group_id: "group_live", state: "running" }],
			});
			expect(discovered.groups).toHaveLength(1);
			expect(with_terminal.groups.map((group) => group.group_id)).toEqual([
				"group_live",
				"group_terminal",
			]);
			expect(updates.updates).toMatchObject([
				{ type: "snapshot", snapshot: { groups: [{ group_id: "group_live" }] } },
				{ type: "patch", snapshot: { groups: [{ group_id: "group_live" }] } },
			]);
			expect(updates.updates[1]?.snapshot.journal_sequence).toBe(
				updates.own.journal_sequence,
			);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("clears an active group-list subscription when its thread is erased", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const database = await runtime.runPromise(Database);
		const erasure = await runtime.runPromise(ThreadErasure);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
		try {
			const created_thread_groups_erased = await Effect.runPromise(
				harness.client.CreateThread({ title: "Erase groups" }),
			);
			await Effect.runPromise(
				database.client.insert(OrchestrationGroups).values({
					group_id: "group_erased",
					thread_id: created_thread_groups_erased.thread_id,
					coordinator_agent_id: "agent_erased",
					state: "running",
					max_concurrency: 1,
					version: 1,
					journal_sequence: 2,
					created_at: "2026-07-18T10:00:00.000Z",
					updated_at: "2026-07-18T10:00:00.000Z",
				}),
			);
			const updates = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const stream = yield* harness.client.SubscribeOrchestrationGroups(
							created_thread_groups_erased.thread_id,
							false,
						);
						const fiber = yield* stream.pipe(
							Stream.take(2),
							Stream.runCollect,
							Effect.forkScoped,
						);
						yield* harness.client.Command({
							command_id: "erase_groups_archive",
							payload: { type: "thread.archive" },
							thread_id: created_thread_groups_erased.thread_id,
						});
						yield* erasure.CleanupExpired(
							"2026-07-19T00:00:00.000Z",
							"2026-07-19T00:00:00.000Z",
						);
						return [...(yield* Fiber.join(fiber))];
					}),
				),
			);
			expect(updates).toMatchObject([
				{ type: "snapshot", snapshot: { groups: [{ group_id: "group_erased" }] } },
				{ type: "patch", snapshot: { groups: [] } },
			]);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});
	it("projects a multi-source project rehome through real MessagePorts", async () => {
		const database_path = await make_database_path();
		const now = { value: "2026-07-11T18:00:00.000Z" };
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			project_locator: make_project_locator_layer(),
			runtime_metadata: make_metadata_layer(now),
			thread_metadata_refiner: make_thread_metadata_refiner_test_layer((input) =>
				Effect.succeed({
					live_status: "Working",
					...(input.recent_user_text.at(-1)?.includes("Beta repository")
						? { mentioned_projects: [ProjectBeta] }
						: {}),
				}),
			),
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const coordinator = await runtime.runPromise(ThreadProjectAffinityCoordinator);
		const database = await runtime.runPromise(Database);
		const journal = await runtime.runPromise(JournalStore);
		const metadata_coordinator = await runtime.runPromise(ThreadMetadataRefinementCoordinator);
		const workspace_evidence = await runtime.runPromise(WorkspaceEvidenceRecorder);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
		try {
			const result = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const updates = yield* harness.client.SubscribeThreadList;
						const rehomed = yield* updates.pipe(
							Stream.filter(
								(update) =>
									update.type === "upsert" &&
									update.thread.primary_project?.project_id ===
										ProjectBeta.project_id,
							),
							Stream.runHead,
							Effect.forkScoped,
						);

						const created_thread = yield* harness.client.CreateThread({
							title: "Cross-repository public projection",
						});
						const thread_id = created_thread.thread_id;
						yield* journal.AppendEvent({
							causation_id: "alpha_run_cause",
							correlation_id: "alpha_run_correlation",
							payload: {
								state: "running",
								type: "run.lifecycle",
								working_directory: ProjectAlpha.root_path,
							},
							run_id: "alpha_run",
							thread_id,
						});
						yield* coordinator.CatchUp;

						now.value = "2026-07-11T18:01:00.000Z";
						yield* workspace_evidence.RecordFilesystemMutation({
							destination_path: `${ProjectBeta.root_path}/src/new.ts`,
							operation: "rename",
							operation_id: "beta_filesystem",
							path: `${ProjectBeta.root_path}/src/old.ts`,
							thread_id,
						});
						yield* workspace_evidence.RecordProcessOwnership({
							operation_id: "beta_process",
							source: "artisan_tool",
							thread_id,
							working_directory: ProjectBeta.root_path,
						});
						yield* workspace_evidence.RecordGitWorkspaceObserved({
							branch: "feature/beta",
							changed_file_count: 4,
							has_diff: true,
							operation_id: "beta_git",
							root_path: ProjectBeta.root_path,
							thread_id,
							worktree_path: `${ProjectBeta.root_path}/.worktrees/feature-beta`,
						});
						yield* journal.AppendEvent({
							causation_id: "beta_mention_cause",
							correlation_id: "beta_mention_correlation",
							payload: {
								mentioned_projects: [ProjectBeta],
								message_id: "beta_mention_message",
								reason: "no_active_run",
								text: "Continue in the selected Beta repository.",
								type: "thread.message_queued",
								working_directory: ProjectBeta.root_path,
							},
							thread_id,
						});
						yield* metadata_coordinator.WaitForIdle;
						yield* coordinator.CatchUp;

						return {
							projection: (yield* harness.client.ListThreads)[0]!,
							update: Option.getOrThrow(yield* Fiber.join(rehomed)),
						};
					}),
				),
			);
			const evidence = await Effect.runPromise(
				database.client.select().from(ThreadProjectAffinityEvidence),
			);
			const persisted = JSON.stringify(evidence);

			expect(result.update).toMatchObject({
				thread: {
					linked_projects: [ProjectAlpha],
					primary_project: ProjectBeta,
				},
				type: "upsert",
			});
			expect(result.projection).toMatchObject({
				linked_projects: [ProjectAlpha],
				primary_project: ProjectBeta,
			});
			expect(persisted).not.toContain("Continue in the selected Beta repository.");
			expect(persisted).not.toContain("feature/beta");
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("reconciles Model Behaviour through real MessagePorts without persisting config content", async () => {
		const database_path = await make_database_path();
		const root = dirname(database_path);
		const config_path = join(root, "codex home", "config.toml");
		const files = await Effect.runPromise(
			Effect.service(ModelBehaviourConfigFiles).pipe(
				Effect.provide(make_model_behaviour_config_files_layer()),
			),
		);
		const codex_mapping = make_codex_auto_compaction_mapping({
			installed_version: "0.142.5",
			mapping_available: true,
		});
		const registry = make_model_behaviour_provider_registry_layer([
			make_codex_model_behaviour_provider({
				backups_directory: join(root, "model-behaviour", "backups"),
				files,
				mapping: codex_mapping,
				target_path: config_path,
			}),
			make_inactive_model_behaviour_provider(
				make_unsupported_auto_compaction_mapping(
					"claude",
					"Claude Code has no equivalent supported mapping.",
				),
			),
		]);
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			model_behaviour_provider_registry: registry,
			runtime_metadata: make_metadata_layer({ value: "2026-07-10T18:00:00.000Z" }),
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const database = await runtime.runPromise(Database);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
		const original = '# retained\napi_key = "secret-never-in-sqlite"\n';

		try {
			await fs.mkdir(dirname(config_path), { recursive: true });
			await fs.writeFile(config_path, original, "utf8");

			const initial = await Effect.runPromise(harness.client.GetModelBehaviour);
			const accepted = await Effect.runPromise(
				harness.client.UpdateModelBehaviour({
					command_id: "real_model_behaviour_update",
					setting_id: "auto_compaction_trigger_tokens",
					value: { type: "integer", value: 250_000 },
				}),
			);
			const duplicate = await Effect.runPromise(
				harness.client.UpdateModelBehaviour({
					command_id: "real_model_behaviour_update",
					setting_id: "auto_compaction_trigger_tokens",
					value: { type: "integer", value: 250_000 },
				}),
			);
			const changed_intent = await Effect.runPromise(
				harness.client
					.UpdateModelBehaviour({
						command_id: "real_model_behaviour_update",
						setting_id: "auto_compaction_trigger_tokens",
						value: { type: "integer", value: 300_000 },
					})
					.pipe(Effect.flip),
			);
			const updated = await Effect.runPromise(harness.client.GetModelBehaviour);
			const updated_content = await readFile(config_path, "utf8");

			await fs.writeFile(
				config_path,
				`${updated_content.replace("250000", "300000")}external = true\n`,
				"utf8",
			);
			const drifted = await Effect.runPromise(harness.client.GetModelBehaviour);
			const codex_drift = drifted.providers.find(
				({ provider_id }) => provider_id === "codex",
			)!;
			const ignored = await Effect.runPromise(
				harness.client.ResolveModelBehaviourDrift({
					action: "ignore",
					command_id: "real_model_behaviour_ignore",
					observed_hash: codex_drift.observed_hash!,
					provider_id: "codex",
					setting_id: "auto_compaction_trigger_tokens",
				}),
			);
			const ignored_snapshot = await Effect.runPromise(harness.client.GetModelBehaviour);
			const persisted = await Effect.runPromise(
				Effect.all({
					commands: database.client.select().from(JournalCommands),
					events: database.client.select().from(JournalEvents),
					providers: database.client.select().from(ModelBehaviourProviderStates),
					settings: database.client.select().from(ModelBehaviourSettings),
				}),
			);
			const persisted_json = JSON.stringify(persisted);

			expect(initial.settings[0]!.value).toEqual({ type: "provider_default" });
			expect(accepted.status).toBe("accepted");
			expect(duplicate).toMatchObject({
				journal_sequence: accepted.journal_sequence,
				status: "duplicate",
			});
			expect(changed_intent).toMatchObject({
				code: "protocol",
				protocol_code: "command.id_conflict",
				retryable: false,
			});
			expect(updated.settings[0]!.value).toEqual({ type: "integer", value: 250_000 });
			expect(updated_content).toContain('api_key = "secret-never-in-sqlite"');
			expect(updated_content).toContain("model_auto_compact_token_limit = 250000");
			expect(codex_drift.status).toBe("drift_detected");
			expect(ignored.status).toBe("accepted");
			expect(
				ignored_snapshot.providers.find(({ provider_id }) => provider_id === "codex")!
					.status,
			).toBe("drift_ignored");
			expect(persisted_json).not.toContain("secret-never-in-sqlite");
			expect(persisted.settings).toHaveLength(1);
			expect(persisted.providers).toHaveLength(2);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	}, 30_000);

	it("replays offline Model Behaviour events once and in order after reconnect", async () => {
		const database_path = await make_database_path();
		const root = dirname(database_path);
		const config_path = join(root, "codex home", "config.toml");
		const files = await Effect.runPromise(
			Effect.service(ModelBehaviourConfigFiles).pipe(
				Effect.provide(make_model_behaviour_config_files_layer()),
			),
		);
		const codex_mapping = make_codex_auto_compaction_mapping({
			installed_version: "0.142.5",
			mapping_available: true,
		});
		const registry = make_model_behaviour_provider_registry_layer([
			make_codex_model_behaviour_provider({
				backups_directory: join(root, "model-behaviour", "backups"),
				files,
				mapping: codex_mapping,
				target_path: config_path,
			}),
			make_inactive_model_behaviour_provider(
				make_unsupported_auto_compaction_mapping(
					"claude",
					"Claude Code has no equivalent supported mapping.",
				),
			),
		]);
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			model_behaviour_provider_registry: registry,
			runtime_metadata: make_metadata_layer({ value: "2026-07-10T18:30:00.000Z" }),
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const service = await runtime.runPromise(ModelBehaviourService);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server, {
			client: { reconnect_delay_ms: 100 },
		});

		try {
			const initial_events = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const fiber = yield* harness.client.Events.pipe(
							Stream.filter((event) =>
								event.payload.type.startsWith("model_behaviour."),
							),
							Stream.take(2),
							Stream.runCollect,
							Effect.forkScoped,
						);

						yield* harness.client.GetModelBehaviour;

						return yield* Fiber.join(fiber);
					}),
				),
			);
			const replayed = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const fiber = yield* harness.client.Events.pipe(
							Stream.filter((event) =>
								event.payload.type.startsWith("model_behaviour."),
							),
							Stream.take(3),
							Stream.runCollect,
							Effect.forkScoped,
						);

						yield* Effect.sync(harness.close_current_connection);
						yield* service.Update({
							message_id: "offline_model_behaviour_update",
							origin: "frontend",
							sent_at: "2026-07-10T18:30:00.000Z",
							setting_id: "auto_compaction_trigger_tokens",
							value: { type: "integer", value: 250_000 },
						});
						yield* Effect.promise(() =>
							wait_for(() => harness.connector_snapshot().connections >= 2),
						);

						return yield* Fiber.join(fiber);
					}),
				),
			);
			const initial = Array.from(initial_events);
			const events = Array.from(replayed);

			expect(initial.map((event) => event.payload.type)).toEqual([
				"model_behaviour.provider.reconciled",
				"model_behaviour.provider.reconciled",
			]);
			expect(events.map((event) => event.payload.type)).toEqual([
				"model_behaviour.setting.updated",
				"model_behaviour.provider.reconciled",
				"model_behaviour.provider.reconciled",
			]);
			expect(events.map((event) => event.journal_sequence)).toEqual(
				[...events]
					.map((event) => event.journal_sequence)
					.sort((left, right) => left - right),
			);
			expect(new Set(events.map((event) => event.message_id)).size).toBe(3);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("queries and updates global guidance through real MessagePorts", async () => {
		const database_path = await make_database_path();
		const canonical_path = join(dirname(database_path), "guidance", "GLOBAL.md");
		const runtime = make_backend_runtime({
			database_path,
			guidance: { canonical_path },
			migrations_path,
			runtime_metadata: make_metadata_layer({ value: "2026-07-10T18:00:00.000Z" }),
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const database = await runtime.runPromise(Database);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);

		try {
			const initial = await Effect.runPromise(harness.client.GetGlobalGuidance);
			const receipt = await Effect.runPromise(
				harness.client.UpdateGlobalGuidance({
					command_id: "real_guidance_update_1",
					content: "Real transport guidance\n",
				}),
			);
			const updated = await Effect.runPromise(harness.client.GetGlobalGuidance);
			const canonical_file = await readFile(canonical_path, "utf8");
			const metadata_rows = await Effect.runPromise(
				Effect.gen(function* () {
					const canonical = yield* database.client.select().from(GlobalGuidanceCanonical);
					const providers = yield* database.client
						.select()
						.from(GlobalGuidanceProviderSync);
					const commands = yield* database.client.select().from(JournalCommands);
					const events = yield* database.client.select().from(JournalEvents);

					return { canonical, commands, events, providers };
				}),
			);
			const query_event_count = metadata_rows.events.length;
			const queried = await Effect.runPromise(harness.client.GetGlobalGuidance);
			const after_query_events = await Effect.runPromise(
				database.client.select().from(JournalEvents),
			);
			const changed_content = await Effect.runPromise(
				harness.client
					.UpdateGlobalGuidance({
						command_id: "real_guidance_update_1",
						content: "Changed real transport guidance\n",
					})
					.pipe(Effect.flip),
			);
			const after_conflict = await Effect.runPromise(harness.client.GetGlobalGuidance);
			const after_conflict_file = await readFile(canonical_path, "utf8");
			const after_conflict_rows = await Effect.runPromise(
				Effect.gen(function* () {
					return {
						canonical: yield* database.client.select().from(GlobalGuidanceCanonical),
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
					};
				}),
			);
			const journal_rows = JSON.stringify({
				commands: metadata_rows.commands,
				events: metadata_rows.events,
			});

			expect(initial.content).toBe("");
			expect(receipt).toMatchObject({
				command_id: "real_guidance_update_1",
				status: "accepted",
			});
			expect(updated.content).toBe("Real transport guidance\n");
			expect(canonical_file).toBe("Real transport guidance\n");
			expect(metadata_rows.canonical).toHaveLength(1);
			expect(metadata_rows.canonical[0]).toMatchObject({
				byte_count: Buffer.byteLength("Real transport guidance\n"),
				content_hash: updated.metadata.canonical.content_hash,
				status: "ready",
			});
			expect(metadata_rows.providers).toEqual([]);
			expect(metadata_rows.commands).toHaveLength(2);
			expect(metadata_rows.events).toHaveLength(2);
			expect(metadata_rows.events.map((event) => event.stream_id)).toEqual([
				"settings:guidance",
				"settings:guidance",
			]);
			expect(journal_rows).toContain(updated.metadata.canonical.content_hash);
			expect(journal_rows).not.toContain("Real transport guidance");
			expect(queried).toEqual(updated);
			expect(after_query_events).toHaveLength(query_event_count);
			expect(changed_content).toMatchObject({
				code: "protocol",
				protocol_code: "command.id_conflict",
				retryable: false,
			});
			expect(after_conflict).toEqual(updated);
			expect(after_conflict_file).toBe(canonical_file);
			expect(after_conflict_rows.commands).toHaveLength(metadata_rows.commands.length);
			expect(after_conflict_rows.events).toHaveLength(metadata_rows.events.length);
			expect(after_conflict_rows.canonical).toEqual(metadata_rows.canonical);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("rejects a stale first-run source and accepts an exact provider candidate", async () => {
		const database_path = await make_database_path();
		const root = dirname(database_path);
		const canonical_path = join(root, "guidance", "GLOBAL.md");
		const codex_content = "Codex candidate guidance\n";
		const claude_content = "Claude candidate guidance\n";
		const candidate = (provider: "claude" | "codex", content: string, path: string) => ({
			Discover: Effect.succeed({
				_tag: "Present" as const,
				content,
				hash: createHash("sha256").update(content).digest("hex"),
				modified_at: "2026-07-10T17:00:00.000Z",
				path,
			}),
			mode: "native_file" as const,
			provider,
		});
		const runtime = make_backend_runtime({
			database_path,
			guidance: { canonical_path },
			guidance_provider_registry: make_guidance_provider_registry_layer([
				candidate("codex", codex_content, join(root, "providers", "AGENTS.md")),
				candidate("claude", claude_content, join(root, "providers", "CLAUDE.md")),
			]),
			migrations_path,
			runtime_metadata: make_metadata_layer({ value: "2026-07-10T18:00:00.000Z" }),
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const database = await runtime.runPromise(Database);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);

		try {
			const initial = await Effect.runPromise(harness.client.GetGlobalGuidance);
			const before_conflict = await Effect.runPromise(
				Effect.all({
					canonical: database.client.select().from(GlobalGuidanceCanonical),
					commands: database.client.select().from(JournalCommands),
					events: database.client.select().from(JournalEvents),
					providers: database.client.select().from(GlobalGuidanceProviderSync),
				}),
			);
			const stale = await Effect.runPromise(
				harness.client
					.SelectGlobalGuidance({
						command_id: "stale_guidance_selection",
						content_hash: "f".repeat(64),
						provider: "codex",
					})
					.pipe(Effect.flip),
			);
			const after_conflict = await Effect.runPromise(
				Effect.all({
					canonical: database.client.select().from(GlobalGuidanceCanonical),
					commands: database.client.select().from(JournalCommands),
					events: database.client.select().from(JournalEvents),
					providers: database.client.select().from(GlobalGuidanceProviderSync),
				}),
			);
			const codex_candidate = initial.candidates.find(
				({ provider }) => provider === "codex",
			)!;
			const accepted = await Effect.runPromise(
				harness.client.SelectGlobalGuidance({
					command_id: "valid_guidance_selection",
					content_hash: codex_candidate.content_hash,
					provider: "codex",
				}),
			);
			const selected = await Effect.runPromise(harness.client.GetGlobalGuidance);
			const persisted = await Effect.runPromise(
				Effect.all({
					canonical: database.client.select().from(GlobalGuidanceCanonical),
					commands: database.client.select().from(JournalCommands),
					events: database.client.select().from(JournalEvents),
					providers: database.client.select().from(GlobalGuidanceProviderSync),
				}),
			);
			const persisted_json = JSON.stringify(persisted);

			expect(initial.metadata.canonical.status).toBe("selection_required");
			expect(initial.candidates).toHaveLength(2);
			expect(stale).toMatchObject({
				code: "protocol",
				protocol_code: "guidance.conflict",
				retryable: false,
			});
			expect(after_conflict).toEqual(before_conflict);
			expect(accepted.status).toBe("accepted");
			expect(selected.content).toBe(codex_content);
			expect(await readFile(canonical_path, "utf8")).toBe(codex_content);
			expect(persisted_json).not.toContain(codex_content.trim());
			expect(persisted_json).not.toContain(claude_content.trim());
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});

	it("replays an interleaved journal through real MessagePorts without resurrecting erased content", async () => {
		const database_path = await make_database_path();
		const now = { value: "2026-07-10T18:00:00.000Z" };
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(now),
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const database = await runtime.runPromise(Database);
		const erasure = await runtime.runPromise(ThreadErasure);
		const journal = await runtime.runPromise(JournalStore);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server, {
			client: { reconnect_delay_ms: 5 },
		});
		let replay_harness:
			| Awaited<ReturnType<typeof make_transport_test_harness_with_protocol_server>>
			| undefined;

		try {
			const output = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const thread_updates = yield* harness.client.SubscribeThreadList;
						const erasure_delivered = yield* Deferred.make<void>();
						const initial_snapshot = yield* Deferred.make<void>();
						const reconnect_snapshot = yield* Deferred.make<void>();
						const removal_delivered = yield* Deferred.make<void>();
						const replayed_kept_event = yield* Deferred.make<void>();
						const updates = yield* Ref.make<ReadonlyArray<ThreadListUpdate>>([]);
						const events = yield* Ref.make<ReadonlyArray<EventEnvelope>>([]);
						let snapshot_count = 0;

						yield* thread_updates.pipe(
							Stream.tap((update) =>
								Effect.gen(function* () {
									yield* Ref.update(updates, (current) => [...current, update]);

									if (update.type === "snapshot") {
										snapshot_count += 1;
										yield* Deferred.succeed(initial_snapshot, undefined).pipe(
											Effect.asVoid,
										);

										if (snapshot_count >= 2) {
											yield* Deferred.succeed(
												reconnect_snapshot,
												undefined,
											).pipe(Effect.asVoid);
										}
									}

									if (
										update.type === "remove" &&
										update.thread_id === erased_thread.thread_id
									) {
										yield* Deferred.succeed(removal_delivered, undefined).pipe(
											Effect.asVoid,
										);
									}
								}),
							),
							Stream.runDrain,
							Effect.forkScoped,
						);
						yield* harness.client.Events.pipe(
							Stream.tap((event) =>
								Effect.gen(function* () {
									yield* Ref.update(events, (current) => [...current, event]);

									if (
										event.payload.type === "thread.erased" &&
										event.thread_id === erased_thread.thread_id
									) {
										yield* Deferred.succeed(erasure_delivered, undefined).pipe(
											Effect.asVoid,
										);
									}

									if (
										event.payload.type === "thread.message_queued" &&
										event.thread_id === kept_thread.thread_id &&
										event.payload.text === "Surviving later event"
									) {
										yield* Deferred.succeed(
											replayed_kept_event,
											undefined,
										).pipe(Effect.asVoid);
									}
								}),
							),
							Stream.runDrain,
							Effect.forkScoped,
						);

						/**
						 * `Events` is an optional hot observation stream. The initial
						 * guidance event may already have advanced the transport cursor
						 * before this observer exists, and must not be retained for it.
						 */
						yield* Effect.yieldNow;
						yield* Deferred.await(initial_snapshot);

						const erased_thread = yield* harness.client.CreateThread({
							title: "Secret erased title",
						});
						const kept_thread = yield* harness.client.CreateThread({
							title: "Surviving thread",
						});

						now.value = "2026-07-10T18:01:00.000Z";
						yield* journal.AppendEvent({
							causation_id: "secret_message_cause",
							correlation_id: "secret_message_correlation",
							payload: {
								message_id: "secret_message",
								reason: "no_active_run",
								text: "Private erased message body",
								type: "thread.message_queued",
								working_directory: "C:/workspace/erased",
							},
							thread_id: erased_thread.thread_id,
						});

						now.value = "2026-07-10T18:02:00.000Z";
						yield* journal.AppendEvent({
							causation_id: "kept_run_cause",
							correlation_id: "kept_run_correlation",
							payload: {
								state: "running",
								type: "run.lifecycle",
								working_directory: "C:/workspace/kept",
							},
							run_id: "kept_run",
							thread_id: kept_thread.thread_id,
						});

						now.value = "2026-07-10T18:03:00.000Z";
						yield* journal.AppendEvent({
							causation_id: "secret_artifact_cause",
							correlation_id: "secret_artifact_correlation",
							payload: {
								artifact: {
									artifact_id: "secret_artifact",
									assignment_id: "secret_assignment",
									content: "Private erased artifact diff",
									created_at: now.value,
									group_id: "secret_group",
									kind: "diff",
									label: "Secret artifact label",
									run_id: "secret_artifact_run",
								},
								group_id: "secret_group",
								type: "artifact.recorded",
							},
							thread_id: erased_thread.thread_id,
						});

						yield* database.client.insert(ThreadErasureClaims).values({
							claimed_at: "2026-07-10T18:04:00.000Z",
							thread_id: erased_thread.thread_id,
						});
						yield* erasure.ResumeClaimed("2026-07-10T18:04:00.000Z");
						yield* Deferred.await(erasure_delivered);
						yield* Deferred.await(removal_delivered);

						yield* Effect.sync(harness.close_current_connection);
						now.value = "2026-07-10T18:05:00.000Z";
						yield* journal.AppendEvent({
							causation_id: "kept_later_cause",
							correlation_id: "kept_later_correlation",
							payload: {
								message_id: "kept_later_message",
								reason: "unsupported",
								text: "Surviving later event",
								type: "thread.message_queued",
								working_directory: "C:/workspace/kept",
							},
							thread_id: kept_thread.thread_id,
						});
						yield* Effect.promise(() =>
							wait_for(() => harness.connector_snapshot().connections >= 2),
						);
						yield* Deferred.await(replayed_kept_event);
						yield* Deferred.await(reconnect_snapshot);

						return {
							cursors: yield* harness.client.Cursors,
							erased_thread_id: erased_thread.thread_id,
							events: yield* Ref.get(events),
							kept_thread_id: kept_thread.thread_id,
							updates: yield* Ref.get(updates),
						};
					}),
				),
			);
			replay_harness = await make_transport_test_harness_with_protocol_server(
				protocol_server,
				{
					client: { reconnect_delay_ms: 5 },
				},
			);
			const current_replay_harness = replay_harness;
			const replayed_threads = await Effect.runPromise(
				current_replay_harness.client.ListThreads,
			);
			const removal_index = output.updates.findIndex(
				(update) =>
					update.type === "remove" && update.thread_id === output.erased_thread_id,
			);
			const serialized_replay = JSON.stringify(replayed_threads);

			expect(output.events.map((event) => event.journal_sequence)).toEqual([
				2, 3, 4, 5, 6, 7, 8,
			]);
			expect(output.events.map((event) => event.sequence)).toEqual([1, 1, 2, 2, 3, 4, 3]);
			expect(output.events.map((event) => event.thread_id)).toEqual([
				output.erased_thread_id,
				output.kept_thread_id,
				output.erased_thread_id,
				output.kept_thread_id,
				output.erased_thread_id,
				output.erased_thread_id,
				output.kept_thread_id,
			]);
			expect(output.updates[0]).toMatchObject({ type: "snapshot", threads: [] });
			expect(output.updates).toContainEqual(
				expect.objectContaining({
					thread: expect.objectContaining({
						activity_version: 1,
						thread_id: output.kept_thread_id,
					}),
					type: "upsert",
				}),
			);
			expect(removal_index).toBeGreaterThan(0);
			expect(output.updates[removal_index]).toMatchObject({
				journal_sequence: 7,
				thread_id: output.erased_thread_id,
				type: "remove",
			});
			expect(output.updates.slice(removal_index + 1)).not.toContainEqual(
				expect.objectContaining({
					thread: expect.objectContaining({ thread_id: output.erased_thread_id }),
				}),
			);
			expect(output.updates.slice(removal_index + 1)).not.toContainEqual(
				expect.objectContaining({
					threads: expect.arrayContaining([
						expect.objectContaining({ thread_id: output.erased_thread_id }),
					]),
				}),
			);
			expect(output.cursors).toEqual({
				event_cursors: [
					{ sequence: 1, stream_id: "settings:guidance" },
					{ sequence: 4, stream_id: `thread:${output.erased_thread_id}` },
					{ sequence: 3, stream_id: `thread:${output.kept_thread_id}` },
				],
				last_journal_sequence: 8,
			});
			expect(replayed_threads.map((thread) => thread.thread_id)).toEqual([
				output.kept_thread_id,
			]);
			expect(serialized_replay).not.toContain("Secret erased title");
			expect(serialized_replay).not.toContain("Private erased message body");
			expect(serialized_replay).not.toContain("Private erased artifact diff");
		} finally {
			await replay_harness?.dispose();
			await harness.dispose();
			await runtime.dispose();
		}
	});
});
