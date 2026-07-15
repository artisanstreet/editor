import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Deferred, Effect, FileSystem, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope } from "@artisan/protocol";
import {
	BrowserInspectionConnector,
	ExternalUrlLauncher,
	make_backend_runtime,
	PreviewBrowserLifecycle,
	ProtocolRouter,
	ThreadErasure,
	type BrowserInspectionSession,
} from "@artisan/backend";
import { Database } from "../../modules/backend/src/persistence/database";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	PreviewBrowserLaunches,
	PreviewInspectionSessions,
	PreviewTargets,
	PreviewTargetRemovalFences,
	ThreadErasureClaims,
	ThreadRetentionPolicies,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const old_activity = "2026-06-01T00:00:00.000Z";
const cleanup_now = "2026-07-15T00:00:00.000Z";

const MakeDatabasePath = Effect.flatMap(FileSystem.FileSystem, (file_system) =>
	file_system.makeTempDirectory({ prefix: "artisan-preview-browser-erasure-" }).pipe(
		Effect.tap((path) => Effect.sync(() => temporary_directories.push(path))),
		Effect.map((path) => `${path}/artisan.db`),
	),
);

interface BrowserControl {
	readonly lifecycle: Array<"detach" | "scope_release">;
	readonly launcher_urls: Array<string>;
	readonly revoke_calls?: Array<{
		readonly connector_id: string;
		readonly inspection_id: string;
	}>;
}

function make_metadata_layer(instance_id = "preview_browser_erasure_test") {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${instance_id}_${++next_id}`),
		Now: Effect.succeed(cleanup_now),
	});
}

function make_browser_layers(control: BrowserControl) {
	const launcher = Layer.succeed(ExternalUrlLauncher, {
		Open: (url: string) => Effect.sync(() => control.launcher_urls.push(url)),
	});
	const connector = Layer.succeed(BrowserInspectionConnector, {
		Attach: () =>
			Effect.gen(function* () {
				const session: BrowserInspectionSession = {
					Detach: Effect.sync(() => control.lifecycle.push("detach")),
					Disconnected: Effect.never,
				};

				return yield* Effect.acquireRelease(Effect.succeed(session), () =>
					Effect.sync(() => control.lifecycle.push("scope_release")),
				);
			}),
		Revoke: (input: { readonly connector_id: string; readonly inspection_id: string }) =>
			Effect.sync(() => control.revoke_calls?.push(input)),
	});

	return { connector, launcher };
}

function browser_command(
	type: "preview.browser.open" | "preview.inspection.attach",
	message_id: string,
	thread_id: string,
	target: {
		readonly project_id: string;
		readonly target_id: string;
		readonly workspace_id: string;
	} = {
		project_id: "project_global",
		target_id: "target_global",
		workspace_id: "workspace_global",
	},
): CommandEnvelope {
	const payload =
		type === "preview.browser.open"
			? {
					project_id: target.project_id,
					target_id: target.target_id,
					type,
					workspace_id: target.workspace_id,
				}
			: {
					connector_id: "connector_fake",
					inspection_id: `inspection_${thread_id}`,
					project_id: target.project_id,
					target_id: target.target_id,
					type,
					workspace_id: target.workspace_id,
				};

	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: old_activity,
		thread_id,
	};
}

afterEach(async () => {
	await Effect.runPromise(
		Effect.forEach(
			temporary_directories.splice(0),
			(path) =>
				Effect.flatMap(FileSystem.FileSystem, (file_system) =>
					file_system.remove(path, { recursive: true }),
				),
			{ discard: true },
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("preview browser and thread erasure", () => {
	it("waits for a foreign live connector before erasing its thread", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const scope_release_started = await Effect.runPromise(Deferred.make<void>());
		const scope_release = await Effect.runPromise(Deferred.make<void>());
		const owner_control: BrowserControl = {
			lifecycle: [],
			launcher_urls: [],
		};
		const observer_control: BrowserControl = {
			lifecycle: [],
			launcher_urls: [],
		};
		const owner_launcher = Layer.succeed(ExternalUrlLauncher, {
			Open: (url: string) => Effect.sync(() => owner_control.launcher_urls.push(url)),
		});
		const owner_connector = Layer.succeed(BrowserInspectionConnector, {
			Attach: () =>
				Effect.gen(function* () {
					const session: BrowserInspectionSession = {
						Detach: Effect.sync(() => owner_control.lifecycle.push("detach")),
						Disconnected: Effect.never,
					};

					return yield* Effect.acquireRelease(Effect.succeed(session), () =>
						Effect.gen(function* () {
							yield* Effect.sync(() => owner_control.lifecycle.push("scope_release"));
							yield* Deferred.succeed(scope_release_started, undefined);
							yield* Deferred.await(scope_release);
						}),
					);
				}),
			Revoke: () => Deferred.await(scope_release),
		});
		const observer_layers = make_browser_layers(observer_control);
		const observer_connector = Layer.succeed(BrowserInspectionConnector, {
			Attach: () => Effect.die("observer must not attach"),
			Revoke: () => Deferred.await(scope_release),
		});
		const preview_browser = {
			connector_timeout_ms: 100,
			inspection_heartbeat_interval_ms: 10,
			launcher_timeout_ms: 100,
			live_inspection_lease_ms: 1_000,
			operation_lease_ms: 1_000,
			operation_poll_interval_ms: 5,
			recovery_interval_ms: 60_000,
			target_removal_lease_ms: 1_000,
			teardown_timeout_ms: 500,
		} as const;
		const owner_runtime = make_backend_runtime({
			browser_inspection_connector: owner_connector,
			database_path,
			external_url_launcher: owner_launcher,
			migrations_path,
			preview_browser,
			runtime_metadata: make_metadata_layer("preview_browser_owner"),
		});
		const observer_runtime = make_backend_runtime({
			browser_inspection_connector: observer_connector,
			database_path,
			external_url_launcher: observer_layers.launcher,
			migrations_path,
			preview_browser,
			runtime_metadata: make_metadata_layer("preview_browser_observer"),
		});

		try {
			await owner_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const router = yield* ProtocolRouter;

					yield* database.client.insert(Threads).values({
						created_at: old_activity,
						last_activity_at: old_activity,
						thread_id: "thread_cross_runtime",
						title: "Cross-runtime browser thread",
						updated_at: old_activity,
					});
					yield* database.client.insert(EventStreams).values({
						last_sequence: 0,
						stream_id: "thread:thread_cross_runtime",
					});
					yield* database.client.insert(PreviewTargets).values({
						created_at_ms: 1,
						generation_id: "target_global_generation",
						project_id: "project_global",
						state: "registered",
						target_id: "target_global",
						updated_at_ms: 1,
						url: "http://localhost:5173/global",
						workspace_id: "workspace_global",
					});
					yield* router.Route(
						browser_command(
							"preview.inspection.attach",
							"attach_cross_runtime",
							"thread_cross_runtime",
						),
					);
					yield* database.client.update(ThreadRetentionPolicies).set({ enabled: false });
				}),
			);

			let erasure_finished = false;
			const erasure = observer_runtime
				.runPromise(
					Effect.flatMap(ThreadErasure, (service) =>
						service.CleanupExpired("2026-07-01T00:00:00.000Z", cleanup_now),
					),
				)
				.then((result) => {
					erasure_finished = true;

					return result;
				});

			await Effect.runPromise(
				Deferred.await(scope_release_started).pipe(Effect.timeout(1_000)),
			);

			expect(erasure_finished).toBe(false);
			const blocked_state = await observer_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					return {
						inspections: yield* database.client
							.select()
							.from(PreviewInspectionSessions),
						retention: yield* database.client.select().from(ThreadRetentionPolicies),
					};
				}),
			);

			expect(blocked_state.inspections).toMatchObject([
				{
					inspection_id: "inspection_thread_cross_runtime",
					state: "attached",
					thread_id: "thread_cross_runtime",
				},
			]);
			expect(blocked_state.retention).toMatchObject([{ enabled: false, policy_id: 1 }]);

			await Effect.runPromise(Deferred.succeed(scope_release, undefined));

			expect(await erasure).toEqual(["thread_cross_runtime"]);
			expect(owner_control.lifecycle).toEqual(["detach", "scope_release"]);
			expect(observer_control.lifecycle).toEqual([]);
			const settled_state = await observer_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					return {
						events: (yield* database.client.select().from(JournalEvents)).filter(
							(event) => event.thread_id === "thread_cross_runtime",
						),
						inspections: yield* database.client
							.select()
							.from(PreviewInspectionSessions),
						retention: yield* database.client.select().from(ThreadRetentionPolicies),
					};
				}),
			);

			expect(settled_state.inspections).toEqual([]);
			expect(settled_state.retention).toMatchObject([{ enabled: false, policy_id: 1 }]);
			expect(settled_state.events.at(-1)).toMatchObject({
				event_type: "thread.erased",
				thread_id: "thread_cross_runtime",
			});
		} finally {
			await Effect.runPromise(Deferred.succeed(scope_release, undefined));
			await Promise.all([owner_runtime.dispose(), observer_runtime.dispose()]);
		}
	});

	it("hard-fences an expired foreign inspection with Revoke before erasing", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const scope_release_started = await Effect.runPromise(Deferred.make<void>());
		const scope_release = await Effect.runPromise(Deferred.make<void>());
		const revoke_started = await Effect.runPromise(Deferred.make<void>());
		const revoke_release = await Effect.runPromise(Deferred.make<void>());
		const owner_control: BrowserControl = { lifecycle: [], launcher_urls: [] };
		const observer_control: BrowserControl = {
			lifecycle: [],
			launcher_urls: [],
			revoke_calls: [],
		};
		const owner_launcher = Layer.succeed(ExternalUrlLauncher, {
			Open: (url: string) => Effect.sync(() => owner_control.launcher_urls.push(url)),
		});
		const owner_connector = Layer.succeed(BrowserInspectionConnector, {
			Attach: () =>
				Effect.gen(function* () {
					const session: BrowserInspectionSession = {
						Detach: Effect.sync(() => owner_control.lifecycle.push("detach")),
						Disconnected: Effect.never,
					};

					return yield* Effect.acquireRelease(Effect.succeed(session), () =>
						Effect.gen(function* () {
							yield* Effect.sync(() => owner_control.lifecycle.push("scope_release"));
							yield* Deferred.succeed(scope_release_started, undefined);
							yield* Deferred.await(scope_release);
						}),
					);
				}),
			Revoke: () => Effect.void,
		});
		const observer_connector = Layer.succeed(BrowserInspectionConnector, {
			Attach: () => Effect.die("observer must not attach"),
			Revoke: (input: { readonly connector_id: string; readonly inspection_id: string }) =>
				Effect.gen(function* () {
					yield* Effect.sync(() => observer_control.revoke_calls?.push(input));
					yield* Deferred.succeed(revoke_started, undefined);
					yield* Deferred.await(revoke_release);
				}),
		});
		const preview_browser = {
			connector_timeout_ms: 100,
			inspection_heartbeat_interval_ms: 60_000,
			launcher_timeout_ms: 100,
			live_inspection_lease_ms: 120_000,
			operation_lease_ms: 1_000,
			operation_poll_interval_ms: 5,
			recovery_interval_ms: 60_000,
			target_removal_lease_ms: 1_000,
			teardown_timeout_ms: 20,
		} as const;
		const owner_runtime = make_backend_runtime({
			browser_inspection_connector: owner_connector,
			database_path,
			external_url_launcher: owner_launcher,
			migrations_path,
			preview_browser,
			runtime_metadata: make_metadata_layer("expired_owner"),
		});
		const observer_runtime = make_backend_runtime({
			browser_inspection_connector: observer_connector,
			database_path,
			external_url_launcher: Layer.succeed(ExternalUrlLauncher, {
				Open: () => Effect.void,
			}),
			migrations_path,
			preview_browser,
			runtime_metadata: make_metadata_layer("expired_observer"),
		});

		try {
			await owner_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const router = yield* ProtocolRouter;

					yield* database.client.insert(Threads).values({
						created_at: old_activity,
						last_activity_at: old_activity,
						thread_id: "thread_expired_foreign",
						title: "Expired foreign inspection",
						updated_at: old_activity,
					});
					yield* database.client.insert(EventStreams).values({
						last_sequence: 0,
						stream_id: "thread:thread_expired_foreign",
					});
					yield* database.client.insert(PreviewTargets).values({
						created_at_ms: 1,
						generation_id: "target_expired_generation",
						project_id: "project_expired",
						state: "registered",
						target_id: "target_expired",
						updated_at_ms: 1,
						url: "http://localhost:5173/expired",
						workspace_id: "workspace_expired",
					});
					yield* router.Route(
						browser_command(
							"preview.inspection.attach",
							"attach_expired_foreign",
							"thread_expired_foreign",
							{
								project_id: "project_expired",
								target_id: "target_expired",
								workspace_id: "workspace_expired",
							},
						),
					);
					yield* database.client.update(ThreadRetentionPolicies).set({ enabled: false });
				}),
			);

			await owner_runtime.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client.update(PreviewInspectionSessions).set({
						lease_expires_at_ms: 0,
					}),
				),
			);

			let recovery_finished = false;
			const recovery = observer_runtime
				.runPromise(
					Effect.flatMap(PreviewBrowserLifecycle, (lifecycle) =>
						lifecycle.Query({
							project_id: "project_expired",
							workspace_id: "workspace_expired",
						}),
					),
				)
				.then((result) => {
					recovery_finished = true;

					return result;
				});

			await Effect.runPromise(Deferred.await(revoke_started).pipe(Effect.timeout(1_000)));
			await Effect.runPromise(Effect.sleep(25));
			expect(recovery_finished).toBe(false);
			expect(observer_control.revoke_calls).toEqual([
				{
					connector_id: "connector_fake",
					inspection_id: "inspection_thread_expired_foreign",
				},
			]);

			await Effect.runPromise(Deferred.succeed(revoke_release, undefined));
			expect((await recovery).inspections).toMatchObject([
				{
					inspection_id: "inspection_thread_expired_foreign",
					reason: "interrupted",
					state: "disconnected",
				},
			]);
			const erased = await observer_runtime.runPromise(
				Effect.flatMap(ThreadErasure, (service) =>
					service.CleanupExpired("2026-07-01T00:00:00.000Z", cleanup_now),
				),
			);

			expect(erased).toEqual(["thread_expired_foreign"]);
		} finally {
			await Effect.runPromise(Deferred.succeed(scope_release, undefined));
			await Effect.runPromise(Deferred.succeed(revoke_release, undefined));
			await Promise.all([owner_runtime.dispose(), observer_runtime.dispose()]);
		}
	});

	it("rejects an unattested current-generation removal fence without revoking live inspection", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const control: BrowserControl = { lifecycle: [], launcher_urls: [], revoke_calls: [] };
		const layers = make_browser_layers(control);
		const runtime = make_backend_runtime({
			browser_inspection_connector: layers.connector,
			database_path,
			external_url_launcher: layers.launcher,
			migrations_path,
			preview_browser: {
				connector_timeout_ms: 100,
				inspection_heartbeat_interval_ms: 10,
				launcher_timeout_ms: 100,
				live_inspection_lease_ms: 1_000,
				operation_lease_ms: 1_000,
				operation_poll_interval_ms: 5,
				recovery_interval_ms: 60_000,
				target_removal_lease_ms: 1_000,
				teardown_timeout_ms: 20,
			},
			runtime_metadata: make_metadata_layer("synthetic_fence"),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const lifecycle = yield* PreviewBrowserLifecycle;
					const router = yield* ProtocolRouter;

					yield* database.client.insert(Threads).values({
						created_at: old_activity,
						last_activity_at: old_activity,
						thread_id: "thread_synthetic_fence",
						title: "Synthetic removal fence",
						updated_at: old_activity,
					});
					yield* database.client.insert(EventStreams).values({
						last_sequence: 0,
						stream_id: "thread:thread_synthetic_fence",
					});
					yield* database.client.insert(PreviewTargets).values({
						created_at_ms: 1,
						generation_id: "target_synthetic_generation",
						project_id: "project_synthetic",
						state: "registered",
						target_id: "target_synthetic",
						updated_at_ms: 1,
						url: "http://localhost:5173/synthetic",
						workspace_id: "workspace_synthetic",
					});
					yield* router.Route(
						browser_command(
							"preview.inspection.attach",
							"attach_synthetic_fence",
							"thread_synthetic_fence",
							{
								project_id: "project_synthetic",
								target_id: "target_synthetic",
								workspace_id: "workspace_synthetic",
							},
						),
					);
					yield* database.client.update(ThreadRetentionPolicies).set({ enabled: false });
					yield* database.client.insert(PreviewTargetRemovalFences).values({
						committed_at_ms: 1,
						message_id: "synthetic_current_fence",
						project_id: "project_synthetic",
						target_generation_id: "target_synthetic_generation",
						target_id: "target_synthetic",
						thread_id: "thread_synthetic_fence",
						workspace_id: "workspace_synthetic",
					});

					const recovery = yield* lifecycle
						.SettleTargetRemovalFence("synthetic_current_fence")
						.pipe(Effect.exit);

					return {
						recovery,
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						fences: yield* database.client.select().from(PreviewTargetRemovalFences),
						inspections: yield* database.client
							.select()
							.from(PreviewInspectionSessions),
						targets: yield* database.client.select().from(PreviewTargets),
					};
				}),
			);

			expect(result.recovery._tag).toBe("Failure");
			expect(result.fences).toMatchObject([{ message_id: "synthetic_current_fence" }]);
			expect(result.inspections).toMatchObject([
				{ inspection_id: "inspection_thread_synthetic_fence", state: "attached" },
			]);
			expect(result.targets).toMatchObject([
				{ generation_id: "target_synthetic_generation" },
			]);
			expect(control.revoke_calls).toEqual([]);
			expect(
				result.commands.some((command) => command.message_id === "synthetic_current_fence"),
			).toBe(false);
			expect(
				result.events.some(
					(event) =>
						event.correlation_id === "synthetic_current_fence" &&
						event.event_type === "preview.target.updated" &&
						JSON.parse(event.payload_json).action === "removed",
				),
			).toBe(false);
		} finally {
			await runtime.dispose();
		}
	});

	it("serializes target removal with live inspection teardown", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const control: BrowserControl = {
			lifecycle: [],
			launcher_urls: [],
		};
		const layers = make_browser_layers(control);
		const runtime = make_backend_runtime({
			browser_inspection_connector: layers.connector,
			database_path,
			external_url_launcher: layers.launcher,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const lifecycle = yield* PreviewBrowserLifecycle;
					const router = yield* ProtocolRouter;

					yield* database.client.insert(Threads).values({
						created_at: old_activity,
						last_activity_at: old_activity,
						thread_id: "thread_remove",
						title: "Removed preview target",
						updated_at: old_activity,
					});
					yield* database.client.insert(EventStreams).values({
						last_sequence: 0,
						stream_id: "thread:thread_remove",
					});
					yield* database.client.insert(PreviewTargets).values({
						created_at_ms: 1,
						generation_id: "target_global_generation",
						project_id: "project_global",
						state: "registered",
						target_id: "target_global",
						updated_at_ms: 1,
						url: "http://localhost:5173/global",
						workspace_id: "workspace_global",
					});
					yield* router.Route(
						browser_command(
							"preview.inspection.attach",
							"attach_remove",
							"thread_remove",
						),
					);
					yield* router.Route({
						kind: "command",
						message_id: "target_remove",
						origin: "frontend",
						payload: {
							project_id: "project_global",
							target_id: "target_global",
							type: "preview.target.remove",
							workspace_id: "workspace_global",
						},
						protocol_version: 1,
						schema_version: 1,
						sent_at: cleanup_now,
						thread_id: "thread_remove",
					});

					return {
						lifecycle: yield* lifecycle.Query({
							project_id: "project_global",
							workspace_id: "workspace_global",
						}),
						targets: yield* database.client.select().from(PreviewTargets),
					};
				}),
			);

			expect(control.lifecycle).toEqual(["detach", "scope_release"]);
			expect(result.targets).toEqual([]);
			expect(result.lifecycle.inspections).toMatchObject([
				{
					inspection_id: "inspection_thread_remove",
					reason: "target_changed",
					state: "disconnected",
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("retains a thread behind a committed target-removal fence until erasure quiescence settles it", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const control: BrowserControl = {
			lifecycle: [],
			launcher_urls: [],
		};
		const layers = make_browser_layers(control);
		const runtime = make_backend_runtime({
			browser_inspection_connector: layers.connector,
			database_path,
			external_url_launcher: layers.launcher,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const erasure = yield* ThreadErasure;
					const router = yield* ProtocolRouter;

					yield* database.client.insert(Threads).values({
						created_at: old_activity,
						last_activity_at: old_activity,
						thread_id: "thread_fenced_removal",
						title: "Fenced removal thread",
						updated_at: old_activity,
					});
					yield* database.client.insert(EventStreams).values({
						last_sequence: 0,
						stream_id: "thread:thread_fenced_removal",
					});
					yield* database.client.insert(PreviewTargets).values({
						created_at_ms: 1,
						generation_id: "target_fenced_generation_1",
						project_id: "project_fenced",
						state: "registered",
						target_id: "target_fenced",
						updated_at_ms: 1,
						url: "http://localhost:5173/fenced-generation-1",
						workspace_id: "workspace_fenced",
					});
					const removal_command = {
						kind: "command",
						message_id: "remove_fenced_generation_1",
						origin: "frontend",
						payload: {
							project_id: "project_fenced",
							target_id: "target_fenced",
							type: "preview.target.remove",
							workspace_id: "workspace_fenced",
						},
						protocol_version: 1,
						schema_version: 1,
						sent_at: cleanup_now,
						thread_id: "thread_fenced_removal",
					} satisfies CommandEnvelope;
					const removal = yield* router.Route(removal_command);
					const removal_event = removal.find(
						(envelope) =>
							envelope.kind === "event" &&
							envelope.payload.type === "preview.target.updated" &&
							envelope.payload.action === "removed",
					);

					if (
						removal_event?.kind !== "event" ||
						removal_event.payload.type !== "preview.target.updated" ||
						removal_event.payload.action !== "removed"
					) {
						return yield* Effect.die(
							"Canonical target removal did not produce its event",
						);
					}

					yield* database.client.insert(PreviewTargetRemovalFences).values({
						committed_at_ms: removal_event.payload.target.updated_at_ms,
						message_id: removal_command.message_id,
						project_id: removal_command.payload.project_id,
						target_generation_id: "target_fenced_generation_1",
						target_id: removal_command.payload.target_id,
						thread_id: removal_command.thread_id,
						workspace_id: removal_command.payload.workspace_id,
					});
					yield* database.client.insert(PreviewTargets).values({
						created_at_ms: 2,
						generation_id: "target_fenced_generation_2",
						project_id: "project_fenced",
						state: "registered",
						target_id: "target_fenced",
						updated_at_ms: 2,
						url: "http://localhost:5173/fenced-generation-2",
						workspace_id: "workspace_fenced",
					});

					const blocked_erasure = yield* erasure.CleanupExpired(
						"2026-07-01T00:00:00.000Z",
						cleanup_now,
					);
					const blocked_state = {
						fences: yield* database.client.select().from(PreviewTargetRemovalFences),
						threads: yield* database.client.select().from(Threads),
					};

					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: cleanup_now,
						thread_id: "thread_fenced_removal",
					});
					const erased = yield* erasure.ResumeClaimed(cleanup_now);

					return {
						blocked_erasure,
						blocked_state,
						erased,
						fences: yield* database.client.select().from(PreviewTargetRemovalFences),
						targets: yield* database.client.select().from(PreviewTargets),
						threads: yield* database.client.select().from(Threads),
					};
				}),
			);

			expect(result.blocked_erasure).toEqual([]);
			expect(result.blocked_state.threads).toMatchObject([
				{ thread_id: "thread_fenced_removal" },
			]);
			expect(result.blocked_state.fences).toMatchObject([
				{
					message_id: "remove_fenced_generation_1",
					target_generation_id: "target_fenced_generation_1",
				},
			]);
			expect(result.erased).toEqual(["thread_fenced_removal"]);
			expect(result.fences).toEqual([]);
			expect(result.threads).toEqual([]);
			expect(result.targets).toMatchObject([
				{
					generation_id: "target_fenced_generation_2",
					target_id: "target_fenced",
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("quiesces and erases one thread while preserving the global target and unrelated browser state", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const control: BrowserControl = {
			lifecycle: [],
			launcher_urls: [],
		};
		const layers = make_browser_layers(control);
		const runtime = make_backend_runtime({
			browser_inspection_connector: layers.connector,
			database_path,
			external_url_launcher: layers.launcher,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const journal = yield* JournalStore;
					const router = yield* ProtocolRouter;
					const erasure = yield* ThreadErasure;

					yield* database.client.insert(Threads).values([
						{
							created_at: old_activity,
							last_activity_at: old_activity,
							thread_id: "thread_erased",
							title: "Erased browser thread",
							updated_at: old_activity,
						},
						{
							created_at: old_activity,
							last_activity_at: cleanup_now,
							thread_id: "thread_kept",
							title: "Surviving browser thread",
							updated_at: old_activity,
						},
					]);
					yield* database.client.insert(EventStreams).values([
						{ last_sequence: 0, stream_id: "thread:thread_erased" },
						{ last_sequence: 0, stream_id: "thread:thread_kept" },
					]);
					yield* database.client.insert(PreviewTargets).values({
						created_at_ms: 1,
						generation_id: "target_global_generation",
						project_id: "project_global",
						state: "registered",
						target_id: "target_global",
						updated_at_ms: 1,
						url: "http://localhost:5173/global",
						workspace_id: "workspace_global",
					});

					yield* router.Route(
						browser_command("preview.browser.open", "open_erased", "thread_erased"),
					);
					yield* router.Route(
						browser_command(
							"preview.inspection.attach",
							"attach_erased",
							"thread_erased",
						),
					);
					yield* router.Route(
						browser_command("preview.browser.open", "open_kept", "thread_kept"),
					);
					yield* router.Route(
						browser_command("preview.inspection.attach", "attach_kept", "thread_kept"),
					);
					yield* journal.AppendEvent({
						causation_id: "secret_browser_event_cause",
						correlation_id: "secret_browser_event_correlation",
						payload: {
							message_id: "secret_browser_message",
							text: "secret browser source payload",
							type: "assistant.message_completed",
						},
						raw_origin: {
							provider: "secret_event_provider",
							reference: "secret_event_source",
						},
						thread_id: "thread_erased",
					});
					const erased = yield* erasure.CleanupExpired(
						"2026-07-01T00:00:00.000Z",
						cleanup_now,
					);

					return {
						erased,
						events: yield* database.client.select().from(JournalEvents),
						commands: yield* database.client.select().from(JournalCommands),
						launches: yield* database.client.select().from(PreviewBrowserLaunches),
						inspections: yield* database.client
							.select()
							.from(PreviewInspectionSessions),
						targets: yield* database.client.select().from(PreviewTargets),
					};
				}),
			);

			expect(result.erased).toEqual(["thread_erased"]);
			expect(control.launcher_urls).toEqual([
				"http://localhost:5173/global",
				"http://localhost:5173/global",
			]);
			expect(control.lifecycle).toEqual(["detach", "scope_release"]);
			expect(result.launches).toMatchObject([
				{ message_id: "open_kept", thread_id: "thread_kept" },
			]);
			expect(result.inspections).toMatchObject([
				{
					inspection_id: "inspection_thread_kept",
					thread_id: "thread_kept",
					state: "attached",
				},
			]);
			expect(result.targets).toMatchObject([
				{ target_id: "target_global", project_id: "project_global" },
			]);
			expect(result.commands.every((command) => command.thread_id !== "thread_erased")).toBe(
				true,
			);
			const erased_events = result.events.filter(
				(event) => event.thread_id === "thread_erased",
			);

			expect(erased_events.at(-1)).toMatchObject({
				event_type: "thread.erased",
				raw_origin_json: null,
			});
			expect(erased_events.every((event) => event.raw_origin_json === null)).toBe(true);
			expect(
				erased_events.every((event) => !event.payload_json.includes("secret_browser")),
			).toBe(true);
			expect(JSON.stringify(result)).not.toContain("secret_browser");
		} finally {
			await runtime.dispose();
		}
	});
});
