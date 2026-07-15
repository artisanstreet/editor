import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Deferred, Effect, Exit, Fiber, FileSystem, Layer, ManagedRuntime, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	CommandEnvelope as Command,
	PreviewBrowserLifecycleQueryResult,
} from "@artisan/protocol";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	PreviewBrowserLaunches,
	PreviewInspectionSessions,
	PreviewTargets,
	ThreadErasureClaims,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import {
	BrowserInspectionConnector,
	ExternalUrlLauncher,
	ExternalUrlLauncherError,
	PreviewBrowserLifecycle,
	type BrowserInspectionSession,
	type PreviewBrowserLifecycleError,
} from "../../modules/backend/src/preview/preview-browser";
import {
	PreviewBrowserRepository,
	PreviewBrowserRepositoryLive,
} from "../../modules/backend/src/preview/preview-browser-repository";
import {
	make_preview_browser_lifecycle_layer,
	type PreviewBrowserLifecycleOptions,
} from "../../modules/backend/src/preview/preview-browser-service";
import { PreviewTargetClock } from "../../modules/backend/src/preview/preview-target";
import {
	PreviewTargetRepository,
	PreviewTargetRepositoryLive,
} from "../../modules/backend/src/preview/preview-target-repository";
import { RuntimeMetadataLive } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const runtimes: Array<{ readonly dispose: () => Promise<void> }> = [];
const paths: Array<string> = [];
const now = "2026-07-15T10:00:00.000Z";
const NoopRevoke = () => Effect.void;

type OpenCommand = Omit<Command, "payload"> & {
	readonly payload: Extract<Command["payload"], { readonly type: "preview.browser.open" }>;
};
type AttachCommand = Omit<Command, "payload"> & {
	readonly payload: Extract<Command["payload"], { readonly type: "preview.inspection.attach" }>;
};
type ProbeCommand = Omit<Command, "payload"> & {
	readonly payload: Extract<Command["payload"], { readonly type: "preview.target.probe" }>;
};
type RegisterCommand = Omit<Command, "payload"> & {
	readonly payload: Extract<Command["payload"], { readonly type: "preview.target.register" }>;
};
type RemoveCommand = Omit<Command, "payload"> & {
	readonly payload: Extract<Command["payload"], { readonly type: "preview.target.remove" }>;
};

interface LauncherControl {
	readonly calls: Array<string>;
	readonly observed_states: Array<string>;
}

interface ConnectorControl {
	readonly attach_calls: Array<string>;
	readonly detach_completed: Map<string, Deferred.Deferred<void>>;
	readonly detach_calls: Array<string>;
	readonly scope_releases: Array<string>;
	readonly disconnected: Map<string, Deferred.Deferred<void>>;
	readonly observer_released: Map<string, Deferred.Deferred<void>>;
	readonly observer_started: Map<string, Deferred.Deferred<void>>;
}

interface ClockControl {
	now_ms: number;
}

const MakeDatabasePath = Effect.flatMap(FileSystem.FileSystem, (file_system) =>
	file_system.makeTempDirectory({ prefix: "artisan-preview-browser-" }).pipe(
		Effect.tap((path) => Effect.sync(() => paths.push(path))),
		Effect.map((path) => `${path}/artisan.db`),
	),
);

function open_command(
	message_id = "open_1",
	target_id = "preview_1",
	thread_id = "thread_1",
): OpenCommand {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload: {
			project_id: "project_1",
			target_id,
			type: "preview.browser.open",
			workspace_id: "workspace_1",
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: now,
		thread_id,
	};
}

function attach_command(
	message_id = "attach_1",
	inspection_id = "inspection_1",
	target_id = "preview_1",
	thread_id = "thread_1",
): AttachCommand {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload: {
			connector_id: "connector_1",
			inspection_id,
			project_id: "project_1",
			target_id,
			type: "preview.inspection.attach",
			workspace_id: "workspace_1",
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: now,
		thread_id,
	};
}

function probe_command(message_id = "probe_1", target_id = "preview_1"): ProbeCommand {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload: {
			project_id: "project_1",
			target_id,
			type: "preview.target.probe",
			workspace_id: "workspace_1",
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: now,
		thread_id: "thread_1",
	};
}

function register_command(message_id = "register_1", target_id = "preview_1"): RegisterCommand {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload: {
			project_id: "project_1",
			target_id,
			type: "preview.target.register",
			url: `http://localhost:5173/${target_id}`,
			workspace_id: "workspace_1",
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: now,
		thread_id: "thread_1",
	};
}

function remove_command(message_id = "remove_1", target_id = "preview_1"): RemoveCommand {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload: {
			project_id: "project_1",
			target_id,
			type: "preview.target.remove",
			workspace_id: "workspace_1",
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: now,
		thread_id: "thread_1",
	};
}

function detach_command(inspection_id = "inspection_1", thread_id = "thread_1"): Command {
	return {
		kind: "command",
		message_id: `detach_${inspection_id}`,
		origin: "frontend",
		payload: {
			inspection_id,
			project_id: "project_1",
			type: "preview.inspection.detach",
			workspace_id: "workspace_1",
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: now,
		thread_id,
	};
}

function make_launcher(
	control: LauncherControl,
	reason: ExternalUrlLauncherError["reason"] | undefined = undefined,
	blocked = false,
) {
	return Layer.effect(
		ExternalUrlLauncher,
		Effect.gen(function* () {
			const database = yield* Database;

			return {
				Open: (url: string) =>
					Effect.gen(function* () {
						const rows = yield* database.client
							.select()
							.from(PreviewBrowserLaunches)
							.pipe(Effect.orDie);

						control.calls.push(url);
						control.observed_states.push(rows[0]?.state ?? "missing");

						if (blocked) {
							return yield* Effect.never;
						}

						if (reason !== undefined) {
							return yield* new ExternalUrlLauncherError({ reason });
						}

						return yield* Effect.void;
					}),
			};
		}),
	);
}

async function make_connector(): Promise<{
	readonly control: ConnectorControl;
	readonly layer: Layer.Layer<BrowserInspectionConnector>;
}> {
	const control: ConnectorControl = {
		attach_calls: [],
		detach_completed: new Map(),
		detach_calls: [],
		disconnected: new Map(),
		observer_released: new Map(),
		observer_started: new Map(),
		scope_releases: [],
	};

	return {
		control,
		layer: Layer.succeed(BrowserInspectionConnector, {
			Revoke: NoopRevoke,
			Attach: ({ inspection_id }) =>
				Effect.gen(function* () {
					const detach_completed = yield* Deferred.make<void>();
					const disconnected = yield* Deferred.make<void>();
					const observer_released = yield* Deferred.make<void>();
					const observer_started = yield* Deferred.make<void>();
					const session: BrowserInspectionSession = {
						Detach: Effect.sync(() => control.detach_calls.push(inspection_id)).pipe(
							Effect.andThen(Deferred.succeed(detach_completed, undefined)),
						),
						Disconnected: Deferred.succeed(observer_started, undefined).pipe(
							Effect.andThen(Deferred.await(disconnected)),
							Effect.ensuring(Deferred.succeed(observer_released, undefined)),
						),
					};

					control.attach_calls.push(inspection_id);
					control.detach_completed.set(inspection_id, detach_completed);
					control.disconnected.set(inspection_id, disconnected);
					control.observer_released.set(inspection_id, observer_released);
					control.observer_started.set(inspection_id, observer_started);

					return yield* Effect.acquireRelease(Effect.succeed(session), () =>
						Effect.sync(() => control.scope_releases.push(inspection_id)),
					);
				}),
		}),
	};
}

function make_runtime(
	database_path: string,
	launcher: Layer.Layer<ExternalUrlLauncher, never, Database>,
	connector: Layer.Layer<BrowserInspectionConnector>,
	options: PreviewBrowserLifecycleOptions = {},
	clock: ClockControl = { now_ms: 10_000 },
) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		JournalNotifierLive,
		RuntimeMetadataLive,
		Layer.succeed(PreviewTargetClock, { Now: Effect.sync(() => clock.now_ms) }),
	);
	const repository = PreviewBrowserRepositoryLive.pipe(Layer.provide(infrastructure));
	const target_repository = PreviewTargetRepositoryLive.pipe(Layer.provide(infrastructure));
	const lifecycle = make_preview_browser_lifecycle_layer({
		...options,
		sliding_event_capacity: 8,
	}).pipe(
		Layer.provide(
			Layer.mergeAll(
				connector,
				launcher.pipe(Layer.provide(infrastructure)),
				repository,
				infrastructure,
			),
		),
	);
	const runtime = ManagedRuntime.make(
		Layer.mergeAll(lifecycle, repository, target_repository, infrastructure),
	);

	runtimes.push(runtime);

	return runtime;
}

const SeedThread = (thread_id = "thread_1") =>
	Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(Threads).values({
			created_at: now,
			thread_id,
			title: "Preview thread",
			title_source: "initial",
			updated_at: now,
		});
		yield* database.client.insert(EventStreams).values({
			last_sequence: 0,
			stream_id: `thread:${thread_id}`,
		});
	});

const SeedTarget = (target_id = "preview_1", generation_id = `${target_id}_generation_1`) =>
	Effect.flatMap(Database, (database) =>
		database.client.insert(PreviewTargets).values({
			created_at_ms: 1,
			generation_id,
			project_id: "project_1",
			state: "registered",
			target_id,
			updated_at_ms: 1,
			url: `http://localhost:5173/${target_id}`,
			workspace_id: "workspace_1",
		}),
	);

function has_forbidden_key(value: unknown): boolean {
	if (Array.isArray(value)) {
		return value.some(has_forbidden_key);
	}

	if (value === null || typeof value !== "object") {
		return false;
	}

	return Object.entries(value).some(
		([key, nested]) =>
			["content", "cookies", "endpoint", "screenshot", "token"].includes(key) ||
			has_forbidden_key(nested),
	);
}

function decode_json_records(values: ReadonlyArray<string | null>): ReadonlyArray<unknown> {
	return values.flatMap((value) => (value === null ? [] : [JSON.parse(value)]));
}

afterEach(async () => {
	await Promise.all(runtimes.splice(0).map((runtime) => runtime.dispose()));
	await Effect.runPromise(
		Effect.forEach(
			paths.splice(0),
			(path) =>
				Effect.flatMap(FileSystem.FileSystem, (file_system) =>
					file_system.remove(path, { recursive: true }),
				),
			{ discard: true },
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("PreviewBrowserLifecycle", () => {
	it("persists the dispatch fence before launching and exact-replays without another handoff", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const launcher: LauncherControl = { calls: [], observed_states: [] };
		const connector = await make_connector();
		const runtime = make_runtime(database_path, make_launcher(launcher), connector.layer);
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const lifecycle = yield* PreviewBrowserLifecycle;

				yield* SeedThread();
				yield* SeedTarget();

				const accepted = yield* lifecycle.Open(open_command());
				const duplicate = yield* lifecycle.Open(open_command());

				return { accepted, duplicate };
			}),
		);

		expect(launcher.observed_states).toEqual(["dispatching"]);
		expect(launcher.calls).toEqual(["http://localhost:5173/preview_1"]);
		expect(result.accepted.event.payload).toMatchObject({ action: "dispatched" });
		expect(result.duplicate).toEqual({ event: result.accepted.event, status: "duplicate" });
	});

	it.each([
		["unavailable", "unavailable", "rejected", "launcher_unavailable"],
		["rejected", "rejected", "rejected", "launcher_rejected"],
		["outcome unknown", "outcome_unknown", "outcome_unknown", "launcher_failed"],
	] as const)(
		"settles a %s launcher outcome durably",
		async (_label, launcher_reason, expected_state, expected_reason) => {
			const database_path = await Effect.runPromise(
				MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
			);
			const launcher: LauncherControl = { calls: [], observed_states: [] };
			const connector = await make_connector();
			const runtime = make_runtime(
				database_path,
				make_launcher(launcher, launcher_reason),
				connector.layer,
			);
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const lifecycle = yield* PreviewBrowserLifecycle;

					yield* SeedThread();
					yield* SeedTarget();

					return yield* lifecycle.Open(open_command());
				}),
			);

			expect(result.event.payload).toMatchObject({
				action: expected_state,
				launch: { reason: expected_reason },
			});
		},
	);

	it("bounds launcher and connector handoffs without claiming an unknown success", async () => {
		const launch_database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const launch_control: LauncherControl = { calls: [], observed_states: [] };
		const launch_connector = await make_connector();
		const launch_runtime = make_runtime(
			launch_database_path,
			make_launcher(launch_control, undefined, true),
			launch_connector.layer,
			{ launcher_timeout_ms: 20 },
		);
		const launch = await launch_runtime.runPromise(
			Effect.gen(function* () {
				const lifecycle = yield* PreviewBrowserLifecycle;

				yield* SeedThread();
				yield* SeedTarget();

				return yield* lifecycle.Open(open_command());
			}),
		);
		const connector_database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const connector_control: ConnectorControl = {
			attach_calls: [],
			detach_completed: new Map(),
			detach_calls: [],
			disconnected: new Map(),
			observer_released: new Map(),
			observer_started: new Map(),
			scope_releases: [],
		};
		const blocked_connector = Layer.succeed(BrowserInspectionConnector, {
			Revoke: NoopRevoke,
			Attach: ({ inspection_id }) =>
				Effect.sync(() => connector_control.attach_calls.push(inspection_id)).pipe(
					Effect.andThen(Effect.never),
				),
		});
		const connector_launcher: LauncherControl = { calls: [], observed_states: [] };
		const connector_runtime = make_runtime(
			connector_database_path,
			make_launcher(connector_launcher),
			blocked_connector,
			{ connector_timeout_ms: 20 },
		);
		const inspection = await connector_runtime.runPromise(
			Effect.gen(function* () {
				const lifecycle = yield* PreviewBrowserLifecycle;

				yield* SeedThread();
				yield* SeedTarget();

				return yield* lifecycle.Attach(attach_command());
			}),
		);

		expect(launch_control.calls).toEqual(["http://localhost:5173/preview_1"]);
		expect(launch.event.payload).toMatchObject({
			action: "outcome_unknown",
			launch: { reason: "launcher_failed", state: "outcome_unknown" },
		});
		expect(connector_control.attach_calls).toEqual(["inspection_1"]);
		expect(inspection.event.payload).toMatchObject({
			action: "failed",
			inspection: { reason: "connector_unavailable", state: "failed" },
		});
	});

	it("contains launcher defects and keeps later handoffs healthy", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		let calls = 0;
		const launcher = Layer.succeed(ExternalUrlLauncher, {
			Open: () =>
				Effect.suspend(() => {
					calls += 1;

					return calls === 1 ? Effect.die("private launcher defect") : Effect.void;
				}),
		});
		const connector = await make_connector();
		const runtime = make_runtime(database_path, launcher, connector.layer);
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const lifecycle = yield* PreviewBrowserLifecycle;

				yield* SeedThread();
				yield* SeedTarget();

				const defective = yield* lifecycle.Open(open_command());
				const healthy = yield* lifecycle.Open(open_command("open_2"));

				return { defective, healthy };
			}),
		);

		expect(result.defective.event.payload).toMatchObject({
			action: "outcome_unknown",
			launch: { reason: "launcher_failed" },
		});
		expect(result.healthy.event.payload).toMatchObject({ action: "dispatched" });
		expect(JSON.stringify(result)).not.toContain("private launcher defect");
	});

	it("contains malformed and defective connector sessions without poisoning the service", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const connector = Layer.succeed(BrowserInspectionConnector, {
			Revoke: NoopRevoke,
			Attach: ({ inspection_id }) =>
				Effect.suspend(() => {
					if (inspection_id === "inspection_defect") {
						return Effect.die("private connector defect");
					}

					if (inspection_id === "inspection_malformed") {
						return Effect.succeed({ Disconnected: Effect.never } as never);
					}

					return Effect.succeed({
						Detach: Effect.void,
						Disconnected:
							inspection_id === "inspection_disconnect_defect"
								? Effect.die("private disconnect defect")
								: Effect.never,
					} satisfies BrowserInspectionSession);
				}),
		});
		const launcher = Layer.succeed(ExternalUrlLauncher, { Open: () => Effect.void });
		const runtime = make_runtime(database_path, launcher, connector);
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const lifecycle = yield* PreviewBrowserLifecycle;

				yield* SeedThread();
				yield* SeedTarget();

				const defective = yield* lifecycle.Attach(
					attach_command("attach_defect", "inspection_defect"),
				);
				const malformed = yield* lifecycle.Attach(
					attach_command("attach_malformed", "inspection_malformed"),
				);
				const disconnected = yield* lifecycle.Attach(
					attach_command("attach_disconnect_defect", "inspection_disconnect_defect"),
				);

				yield* Effect.sleep("30 millis");

				const healthy = yield* lifecycle.Attach(
					attach_command("attach_healthy", "inspection_healthy"),
				);
				const query = yield* lifecycle.Query({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});

				return { defective, disconnected, healthy, malformed, query };
			}),
		);

		expect(result.defective.event.payload).toMatchObject({
			action: "failed",
			inspection: { reason: "connector_rejected" },
		});
		expect(result.malformed.event.payload).toMatchObject({
			action: "failed",
			inspection: { reason: "connector_rejected" },
		});
		expect(result.disconnected.event.payload).toMatchObject({ action: "attached" });
		expect(result.healthy.event.payload).toMatchObject({ action: "attached" });
		expect(result.query.inspections).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					inspection_id: "inspection_disconnect_defect",
					reason: "connection_lost",
					state: "disconnected",
				}),
			]),
		);
		expect(JSON.stringify(result)).not.toContain("private connector defect");
		expect(JSON.stringify(result)).not.toContain("private disconnect defect");
	});

	it("reserves one launch command identity before blocked adapter work across runtimes", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const started = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const calls: Array<string> = [];
		const launcher = Layer.succeed(ExternalUrlLauncher, {
			Open: (url: string) =>
				Effect.gen(function* () {
					calls.push(url);
					yield* Deferred.succeed(started, undefined);
					yield* Deferred.await(release);
				}),
		});
		const connector = await make_connector();
		const first = make_runtime(database_path, launcher, connector.layer, {
			launcher_timeout_ms: 1_000,
		});
		const second = make_runtime(database_path, launcher, connector.layer, {
			launcher_timeout_ms: 1_000,
		});
		await first.runPromise(
			Effect.gen(function* () {
				yield* SeedThread();
				yield* SeedTarget();
			}),
		);
		const first_open = first.runPromise(
			Effect.flatMap(PreviewBrowserLifecycle, (lifecycle) => lifecycle.Open(open_command())),
		);
		await Effect.runPromise(Deferred.await(started));
		const second_open = second.runPromise(
			Effect.flatMap(PreviewBrowserLifecycle, (lifecycle) => lifecycle.Open(open_command())),
		);
		await Effect.runPromise(Deferred.succeed(release, undefined));
		const result = { first_open: await first_open, second_open: await second_open };

		expect(calls).toEqual(["http://localhost:5173/preview_1"]);
		expect(result.second_open).toEqual({
			event: expect.objectContaining({
				payload: expect.objectContaining({ action: "dispatched" }),
			}),
			status: "duplicate",
		});
	});

	it("rejects a different browser command type while the shared identity is pending", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const started = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const launcher = Layer.succeed(ExternalUrlLauncher, {
			Open: () =>
				Effect.gen(function* () {
					yield* Deferred.succeed(started, undefined);
					yield* Deferred.await(release);
				}),
		});
		const connector = await make_connector();
		const first = make_runtime(database_path, launcher, connector.layer, {
			launcher_timeout_ms: 1_000,
		});
		const second = make_runtime(database_path, launcher, connector.layer, {
			launcher_timeout_ms: 1_000,
		});

		await first.runPromise(
			Effect.gen(function* () {
				yield* SeedThread();
				yield* SeedTarget();
			}),
		);

		const open = first.runPromise(
			Effect.flatMap(PreviewBrowserLifecycle, (lifecycle) => lifecycle.Open(open_command())),
		);

		await Effect.runPromise(Deferred.await(started));

		const conflict = await second.runPromise(
			Effect.flatMap(PreviewBrowserLifecycle, (lifecycle) =>
				lifecycle.Attach(attach_command("open_1", "inspection_conflict")),
			).pipe(Effect.flip),
		);

		await Effect.runPromise(Deferred.succeed(release, undefined));
		await open;

		expect(conflict).toMatchObject({ code: "conflict", subject_id: "open_1" });
		expect(connector.control.attach_calls).toEqual([]);
	});

	it("replays one concurrent attach exactly without double connector work or live divergence", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const started = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const attach_calls: Array<string> = [];
		const connector = Layer.succeed(BrowserInspectionConnector, {
			Revoke: NoopRevoke,
			Attach: ({ inspection_id }) =>
				Effect.gen(function* () {
					attach_calls.push(inspection_id);
					yield* Deferred.succeed(started, undefined);
					yield* Deferred.await(release);

					return {
						Detach: Effect.void,
						Disconnected: Effect.never,
					} satisfies BrowserInspectionSession;
				}),
		});
		const launcher = Layer.succeed(ExternalUrlLauncher, { Open: () => Effect.void });
		const first = make_runtime(database_path, launcher, connector, {
			connector_timeout_ms: 1_000,
		});
		const second = make_runtime(database_path, launcher, connector, {
			connector_timeout_ms: 1_000,
		});
		await first.runPromise(
			Effect.gen(function* () {
				yield* SeedThread();
				yield* SeedTarget();
			}),
		);
		const first_attach = first.runPromise(
			Effect.flatMap(PreviewBrowserLifecycle, (lifecycle) =>
				lifecycle.Attach(attach_command()),
			),
		);
		await Effect.runPromise(Deferred.await(started));
		const second_attach = second.runPromise(
			Effect.flatMap(PreviewBrowserLifecycle, (lifecycle) =>
				lifecycle.Attach(attach_command()),
			),
		);
		await Effect.runPromise(Deferred.succeed(release, undefined));
		const result = { first_attach: await first_attach, second_attach: await second_attach };

		expect(attach_calls).toEqual(["inspection_1"]);
		expect(result.second_attach).toEqual({
			event: result.first_attach.event,
			status: "duplicate",
		});
		const query = await first.runPromise(
			Effect.flatMap(PreviewBrowserLifecycle, (lifecycle) =>
				lifecycle.Query({ project_id: "project_1", workspace_id: "workspace_1" }),
			),
		);

		expect(query.inspections).toMatchObject([
			{ inspection_id: "inspection_1", state: "attached" },
		]);
	});

	it("rejects cross-runtime target removal while a launch lease is active", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const started = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const launcher = Layer.succeed(ExternalUrlLauncher, {
			Open: () =>
				Effect.gen(function* () {
					yield* Deferred.succeed(started, undefined);
					yield* Deferred.await(release);
				}),
		});
		const connector = await make_connector();
		const first = make_runtime(database_path, launcher, connector.layer, {
			launcher_timeout_ms: 1_000,
		});
		const second = make_runtime(database_path, launcher, connector.layer, {
			launcher_timeout_ms: 1_000,
		});
		await first.runPromise(
			Effect.gen(function* () {
				yield* SeedThread();
				yield* SeedTarget();
			}),
		);
		const open = first.runPromise(
			Effect.flatMap(PreviewBrowserLifecycle, (lifecycle) => lifecycle.Open(open_command())),
		);
		await Effect.runPromise(Deferred.await(started));
		const removal_error = await second.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const lifecycle = yield* PreviewBrowserLifecycle;

				return yield* lifecycle.SynchronizeTargetRemoval(
					{
						project_id: "project_1",
						target_id: "preview_1",
						workspace_id: "workspace_1",
					},
					() =>
						database.client
							.delete(PreviewTargets)
							.pipe(Effect.as({ status: "accepted" as const })),
				);
			}).pipe(Effect.flip),
		);
		await Effect.runPromise(Deferred.succeed(release, undefined));
		const result = await open;

		expect(removal_error).toMatchObject({ code: "unavailable", subject_id: "preview_1" });
		expect(result.event.payload).toMatchObject({
			action: "dispatched",
			launch: { state: "dispatched" },
		});
	});

	it("rejects cross-runtime target removal while an attach lease is active", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const started = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const attach_calls: Array<string> = [];
		const connector = Layer.succeed(BrowserInspectionConnector, {
			Revoke: NoopRevoke,
			Attach: ({ inspection_id }) =>
				Effect.gen(function* () {
					attach_calls.push(inspection_id);
					yield* Deferred.succeed(started, undefined);
					yield* Deferred.await(release);

					return {
						Detach: Effect.void,
						Disconnected: Effect.never,
					} satisfies BrowserInspectionSession;
				}),
		});
		const launcher = Layer.succeed(ExternalUrlLauncher, { Open: () => Effect.void });
		const first = make_runtime(database_path, launcher, connector, {
			connector_timeout_ms: 1_000,
		});
		const second = make_runtime(database_path, launcher, connector, {
			connector_timeout_ms: 1_000,
		});
		await first.runPromise(
			Effect.gen(function* () {
				yield* SeedThread();
				yield* SeedTarget();
			}),
		);
		const attach = first.runPromise(
			Effect.flatMap(PreviewBrowserLifecycle, (lifecycle) =>
				lifecycle.Attach(attach_command()),
			),
		);
		await Effect.runPromise(Deferred.await(started));
		const removal_error = await second.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const lifecycle = yield* PreviewBrowserLifecycle;

				return yield* lifecycle.SynchronizeTargetRemoval(
					{
						project_id: "project_1",
						target_id: "preview_1",
						workspace_id: "workspace_1",
					},
					() =>
						database.client
							.delete(PreviewTargets)
							.pipe(Effect.as({ status: "accepted" as const })),
				);
			}).pipe(Effect.flip),
		);
		await Effect.runPromise(Deferred.succeed(release, undefined));
		const result = await attach;

		expect(attach_calls).toEqual(["inspection_1"]);
		expect(removal_error).toMatchObject({ code: "unavailable", subject_id: "preview_1" });
		expect(result.event.payload).toMatchObject({
			action: "attached",
			inspection: { state: "attached" },
		});
	});

	it("blocks target registration and probing while a removal claim is live", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const connector = await make_connector();
		const launcher = Layer.succeed(ExternalUrlLauncher, { Open: () => Effect.void });
		const runtime = make_runtime(database_path, launcher, connector.layer);
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const browser_repository = yield* PreviewBrowserRepository;
				const target_repository = yield* PreviewTargetRepository;
				const now_ms = Date.now();

				yield* SeedThread();
				yield* SeedTarget();

				const probe_removal = yield* browser_repository.ClaimTargetRemoval(
					{
						project_id: "project_1",
						target_id: "preview_1",
						workspace_id: "workspace_1",
					},
					now_ms,
					60_000,
				);
				const probe_error = yield* target_repository
					.ClaimProbe(probe_command(), 30_000)
					.pipe(Effect.flip);

				yield* browser_repository.ReleaseTargetRemoval(probe_removal);

				const probe_claim = yield* target_repository.ClaimProbe(probe_command(), 30_000);

				if (probe_claim._tag !== "Acquired") {
					return yield* Effect.die("Probe-removal fixture did not acquire its lease");
				}

				const removal_error = yield* browser_repository
					.ClaimTargetRemoval(
						{
							project_id: "project_1",
							target_id: "preview_1",
							workspace_id: "workspace_1",
						},
						now_ms,
						60_000,
					)
					.pipe(Effect.flip);

				yield* target_repository.ReleaseProbe(probe_claim.claim);

				const register_removal = yield* browser_repository.ClaimTargetRemoval(
					{
						project_id: "project_1",
						target_id: "preview_2",
						workspace_id: "workspace_1",
					},
					now_ms,
					60_000,
				);
				const registration_error = yield* target_repository
					.Register(
						register_command("register_2", "preview_2"),
						"http://localhost:5173/preview_2",
						now_ms,
					)
					.pipe(Effect.flip);

				yield* browser_repository.ReleaseTargetRemoval(register_removal);

				const registration = yield* target_repository.Register(
					register_command("register_2", "preview_2"),
					"http://localhost:5173/preview_2",
					now_ms,
				);

				return { probe_error, registration, registration_error, removal_error };
			}),
		);

		expect(result.probe_error).toMatchObject({ reason: "target_removing" });
		expect(result.registration_error).toMatchObject({ reason: "target_removing" });
		expect(result.removal_error).toMatchObject({ reason: "target_removing" });
		expect(result.registration.event.payload).toMatchObject({ action: "registered" });
	});

	it("rejects a 257th active inspection in one thread and target", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const connector = await make_connector();
		const launcher: LauncherControl = { calls: [], observed_states: [] };
		const runtime = make_runtime(database_path, make_launcher(launcher), connector.layer, {
			recovery_interval_ms: 60_000,
		});
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const repository = yield* PreviewBrowserRepository;
				const commands = Array.from({ length: 256 }, (_, index) =>
					attach_command(`attach_${index}`, `inspection_${index}`),
				);

				yield* SeedThread();
				yield* SeedTarget();
				yield* Effect.forEach(
					commands,
					(command, index) =>
						Effect.gen(function* () {
							const preparation = yield* repository.PrepareInspection(
								command,
								index,
								30_000,
							);

							if (preparation._tag !== "Prepared") {
								return yield* Effect.die(
									"Inspection capacity fixture was not prepared",
								);
							}

							yield* repository.SettleInspectionAttach(
								command,
								preparation.prepared.claim,
								{ state: "attached" },
								index,
							);
						}),
					{ concurrency: 1, discard: true },
				);
				const capacity_error = yield* repository
					.PrepareInspection(attach_command("attach_256", "inspection_256"), 256, 30_000)
					.pipe(Effect.flip);
				const query = yield* repository.List({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});

				return { capacity_error, query };
			}),
		);

		expect(result.capacity_error).toMatchObject({ reason: "capacity" });
		expect(result.query.inspections).toHaveLength(256);
		expect(
			result.query.inspections.every((inspection) => inspection.state === "attached"),
		).toBe(true);
	});

	it("completes target removal after the connector scope hard-closes a hanging detach", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const teardown_started = await Effect.runPromise(Deferred.make<void>());
		const teardown_interrupted = await Effect.runPromise(Deferred.make<void>());
		const scope_released = await Effect.runPromise(Deferred.make<void>());
		const connector = Layer.succeed(BrowserInspectionConnector, {
			Revoke: NoopRevoke,
			Attach: () =>
				Effect.acquireRelease(
					Effect.succeed({
						Detach: Effect.gen(function* () {
							yield* Deferred.succeed(teardown_started, undefined);
							yield* Effect.never;
						}).pipe(
							Effect.onInterrupt(() =>
								Deferred.succeed(teardown_interrupted, undefined),
							),
						),
						Disconnected: Effect.never,
					} satisfies BrowserInspectionSession),
					() => Deferred.succeed(scope_released, undefined),
				),
		});
		const launcher = Layer.succeed(ExternalUrlLauncher, { Open: () => Effect.void });
		const runtime = make_runtime(database_path, launcher, connector, {
			connector_timeout_ms: 20,
			teardown_timeout_ms: 20,
		});
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const lifecycle = yield* PreviewBrowserLifecycle;

				yield* SeedThread();
				yield* SeedTarget();
				yield* lifecycle.Attach(attach_command());
				const removal = yield* lifecycle
					.SynchronizeTargetRemoval(
						{
							project_id: "project_1",
							target_id: "preview_1",
							workspace_id: "workspace_1",
						},
						() =>
							database.client
								.delete(PreviewTargets)
								.pipe(Effect.as({ status: "accepted" as const })),
					)
					.pipe(Effect.forkChild({ startImmediately: true }));
				yield* Deferred.await(teardown_started);
				const completed_before_release = yield* Fiber.await(removal).pipe(
					Effect.timeout(100),
				);

				if (!Exit.isSuccess(completed_before_release)) {
					return yield* Effect.die(
						"Target removal did not return after teardown timeout",
					);
				}

				yield* Deferred.await(scope_released).pipe(Effect.timeout(100));
				yield* Deferred.await(teardown_interrupted).pipe(Effect.timeout(100));
				const query = yield* lifecycle.Query({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});

				return {
					completed_before_release,
					query,
				};
			}),
		);

		expect(Exit.isSuccess(result.completed_before_release)).toBe(true);
		expect(result.query.inspections).toMatchObject([
			{
				inspection_id: "inspection_1",
				reason: "target_changed",
				state: "disconnected",
			},
		]);
	});

	it("keeps a target-removal fence pending while its connector scope finalizer drains", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const finalizer_started = await Effect.runPromise(Deferred.make<void>());
		const finalizer_release = await Effect.runPromise(Deferred.make<void>());
		const connector = Layer.succeed(BrowserInspectionConnector, {
			Revoke: () => Deferred.await(finalizer_release),
			Attach: () =>
				Effect.acquireRelease(
					Effect.succeed({
						Detach: Effect.void,
						Disconnected: Effect.never,
					} satisfies BrowserInspectionSession),
					() =>
						Effect.gen(function* () {
							yield* Deferred.succeed(finalizer_started, undefined);
							yield* Deferred.await(finalizer_release);
						}),
				),
		});
		const launcher = Layer.succeed(ExternalUrlLauncher, { Open: () => Effect.void });
		const runtime = make_runtime(database_path, launcher, connector, {
			teardown_timeout_ms: 20,
		});
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const repository = yield* PreviewBrowserRepository;
				const lifecycle = yield* PreviewBrowserLifecycle;
				const target_repository = yield* PreviewTargetRepository;
				const command = remove_command("remove_hanging_scope_finalizer");
				const target = {
					project_id: "project_1",
					target_id: "preview_1",
					workspace_id: "workspace_1",
				} as const;

				yield* SeedThread();
				yield* SeedTarget();
				yield* lifecycle.Attach(attach_command());
				const synchronization_error = yield* lifecycle
					.SynchronizeTargetRemoval(target, (claim) =>
						target_repository.RemoveClaimed(command, claim, 10),
					)
					.pipe(Effect.flip);
				yield* Deferred.await(finalizer_started).pipe(Effect.timeout(100));
				const targets_before_retry = yield* database.client.select().from(PreviewTargets);
				const fences_before_retry = yield* repository.ListTargetRemovalFences();
				const inspection_before_retry = yield* lifecycle.Query({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});

				yield* Deferred.succeed(finalizer_release, undefined);
				yield* lifecycle.SettleTargetRemovalFence(command.message_id);
				const targets_after_retry = yield* database.client.select().from(PreviewTargets);
				const fences_after_retry = yield* repository.ListTargetRemovalFences();
				const inspection_after_retry = yield* lifecycle.Query({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});
				const events = yield* database.client.select().from(JournalEvents);

				return {
					events,
					fences_after_retry,
					fences_before_retry,
					inspection_after_retry,
					inspection_before_retry,
					synchronization_error,
					targets_after_retry,
					targets_before_retry,
				};
			}),
		);

		expect(result.synchronization_error).toMatchObject({ code: "unavailable" });
		expect(result.targets_before_retry).toEqual([]);
		expect(result.fences_before_retry).toMatchObject([
			expect.objectContaining({ message_id: "remove_hanging_scope_finalizer" }),
		]);
		expect(result.inspection_before_retry.inspections).toMatchObject([
			{ inspection_id: "inspection_1", state: "attached" },
		]);
		expect(result.targets_after_retry).toEqual([]);
		expect(result.fences_after_retry).toEqual([]);
		expect(result.inspection_after_retry.inspections).toMatchObject([
			{
				inspection_id: "inspection_1",
				reason: "target_changed",
				state: "disconnected",
			},
		]);
		expect(result.events.map((event) => JSON.parse(event.payload_json))).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					action: "disconnected",
					inspection: expect.objectContaining({ reason: "target_changed" }),
				}),
			]),
		);
	});

	it("does not durably fail a timed scoped attach before connector revocation", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const revocation_started = await Effect.runPromise(Deferred.make<void>());
		const revocation_release = await Effect.runPromise(Deferred.make<void>());
		const scope_finalizer_started = await Effect.runPromise(Deferred.make<void>());
		const scope_finalizer_release = await Effect.runPromise(Deferred.make<void>());
		const revocation_calls: Array<string> = [];
		const connector = Layer.succeed(BrowserInspectionConnector, {
			Attach: () =>
				Effect.acquireRelease(Effect.void, () =>
					Effect.gen(function* () {
						yield* Deferred.succeed(scope_finalizer_started, undefined);
						yield* Deferred.await(scope_finalizer_release);
					}),
				).pipe(Effect.andThen(Effect.never)),
			Revoke: ({ connector_id, inspection_id }) =>
				Effect.gen(function* () {
					revocation_calls.push(`${connector_id}:${inspection_id}`);
					yield* Deferred.succeed(revocation_started, undefined);
					yield* Deferred.await(revocation_release);
				}),
		});
		const launcher = Layer.succeed(ExternalUrlLauncher, { Open: () => Effect.void });
		const first = make_runtime(database_path, launcher, connector, {
			connector_timeout_ms: 20,
			teardown_timeout_ms: 20,
		});
		const second = make_runtime(
			database_path,
			launcher,
			connector,
			{
				connector_timeout_ms: 20,
				teardown_timeout_ms: 20,
			},
			{ now_ms: 100_000 },
		);
		const result = await first.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const lifecycle = yield* PreviewBrowserLifecycle;

				yield* SeedThread();
				yield* SeedTarget();
				const attachment = yield* lifecycle
					.Attach(attach_command())
					.pipe(Effect.forkChild({ startImmediately: true }));
				const revocation = yield* Deferred.await(revocation_started).pipe(
					Effect.timeoutOption(100),
				);

				if (Option.isNone(revocation)) {
					yield* Deferred.succeed(revocation_release, undefined);
					yield* Deferred.succeed(scope_finalizer_release, undefined);
					yield* Fiber.await(attachment);

					return yield* Effect.die(
						"Timed scoped attach did not begin connector revocation",
					);
				}

				const attachment_before_revocation = yield* Fiber.await(attachment).pipe(
					Effect.timeoutOption(150),
				);
				const inspections_before_revocation = yield* lifecycle.Query({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});
				const targets_before_revocation = yield* database.client
					.select()
					.from(PreviewTargets);
				const revocation_calls_before_recovery = [...revocation_calls];

				yield* Deferred.succeed(revocation_release, undefined);
				yield* Deferred.succeed(scope_finalizer_release, undefined);

				return {
					attachment_before_revocation,
					inspections_before_revocation,
					revocation,
					revocation_calls_before_recovery,
					scope_finalizer: yield* Deferred.await(scope_finalizer_started).pipe(
						Effect.timeoutOption(100),
					),
					targets_before_revocation,
				};
			}),
		);
		const recovered = await second.runPromise(
			Effect.flatMap(PreviewBrowserLifecycle, (lifecycle) =>
				lifecycle.Query({ project_id: "project_1", workspace_id: "workspace_1" }),
			),
		);

		expect(Option.isSome(result.revocation)).toBe(true);
		expect(Option.isSome(result.scope_finalizer)).toBe(true);
		expect(Option.isSome(result.attachment_before_revocation)).toBe(true);

		if (Option.isSome(result.attachment_before_revocation)) {
			expect(Exit.isFailure(result.attachment_before_revocation.value)).toBe(true);
		}

		expect(result.revocation_calls_before_recovery).toEqual(["connector_1:inspection_1"]);
		expect(revocation_calls.every((call) => call === "connector_1:inspection_1")).toBe(true);
		expect(result.inspections_before_revocation.inspections).toMatchObject([
			{ inspection_id: "inspection_1", state: "attaching" },
		]);
		expect(result.targets_before_revocation).toMatchObject([{ target_id: "preview_1" }]);
		expect(recovered.inspections).toMatchObject([
			{
				inspection_id: "inspection_1",
				reason: "interrupted",
				state: "disconnected",
			},
		]);
	});

	it("keeps an explicit detach attached until connector revocation fences authority", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const revocation_started = await Effect.runPromise(Deferred.make<void>());
		const revocation_release = await Effect.runPromise(Deferred.make<void>());
		const scope_finalizer_started = await Effect.runPromise(Deferred.make<void>());
		const scope_finalizer_release = await Effect.runPromise(Deferred.make<void>());
		const revocation_calls: Array<string> = [];
		const connector = Layer.succeed(BrowserInspectionConnector, {
			Attach: () =>
				Effect.acquireRelease(
					Effect.succeed({
						Detach: Effect.void,
						Disconnected: Effect.never,
					} satisfies BrowserInspectionSession),
					() =>
						Effect.gen(function* () {
							yield* Deferred.succeed(scope_finalizer_started, undefined);
							yield* Deferred.await(scope_finalizer_release);
						}),
				),
			Revoke: ({ connector_id, inspection_id }) =>
				Effect.gen(function* () {
					revocation_calls.push(`${connector_id}:${inspection_id}`);
					yield* Deferred.succeed(revocation_started, undefined);
					yield* Deferred.await(revocation_release);
				}),
		});
		const launcher = Layer.succeed(ExternalUrlLauncher, { Open: () => Effect.void });
		const runtime = make_runtime(database_path, launcher, connector, {
			connector_timeout_ms: 500,
			teardown_timeout_ms: 500,
		});
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const lifecycle = yield* PreviewBrowserLifecycle;

				yield* SeedThread();
				yield* SeedTarget();
				yield* lifecycle.Attach(attach_command());
				const detach = yield* lifecycle
					.Detach(detach_command())
					.pipe(Effect.forkChild({ startImmediately: true }));
				const revocation = yield* Deferred.await(revocation_started).pipe(
					Effect.timeoutOption(100),
				);

				if (Option.isNone(revocation)) {
					yield* Deferred.succeed(scope_finalizer_release, undefined);
					yield* Fiber.await(detach);

					return yield* Effect.die("Explicit detach did not begin connector revocation");
				}

				yield* Effect.sleep(30);
				const detach_before_revocation = yield* Fiber.await(detach).pipe(
					Effect.timeoutOption(30),
				);
				const inspections_before_revocation = yield* lifecycle.Query({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});
				const events_before_revocation = yield* database.client
					.select()
					.from(JournalEvents);

				yield* Deferred.succeed(revocation_release, undefined);
				yield* Deferred.succeed(scope_finalizer_release, undefined);

				return {
					detach: yield* Fiber.join(detach),
					detach_before_revocation,
					events_before_revocation,
					events_after_revocation: yield* database.client.select().from(JournalEvents),
					inspections_after_revocation: yield* lifecycle.Query({
						project_id: "project_1",
						workspace_id: "workspace_1",
					}),
					inspections_before_revocation,
					revocation,
					scope_finalizer: yield* Deferred.await(scope_finalizer_started).pipe(
						Effect.timeoutOption(100),
					),
				};
			}),
		);

		expect(Option.isSome(result.revocation)).toBe(true);
		expect(Option.isSome(result.scope_finalizer)).toBe(true);
		expect(revocation_calls).toEqual(["connector_1:inspection_1"]);
		expect(Option.isNone(result.detach_before_revocation)).toBe(true);
		expect(result.inspections_before_revocation.inspections).toMatchObject([
			{ inspection_id: "inspection_1", state: "attached" },
		]);
		expect(
			result.events_before_revocation.map((event) => JSON.parse(event.payload_json)),
		).toEqual([expect.objectContaining({ action: "attached" })]);
		expect(result.detach.event.payload).toMatchObject({
			action: "disconnected",
			inspection: { reason: "detached", state: "disconnected" },
		});
		expect(
			result.events_after_revocation
				.map((event) => JSON.parse(event.payload_json))
				.filter(
					(event) =>
						event.action === "disconnected" &&
						event.inspection.inspection_id === "inspection_1" &&
						event.inspection.reason === "detached",
				),
		).toHaveLength(1);
		expect(
			result.inspections_after_revocation.inspections.filter(
				(inspection) => inspection.inspection_id === "inspection_1",
			),
		).toMatchObject([{ reason: "detached", state: "disconnected" }]);
	});

	it("does not record an observed disconnect before connector revocation fences authority", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const disconnected = await Effect.runPromise(Deferred.make<void>());
		const revocation_started = await Effect.runPromise(Deferred.make<void>());
		const revocation_release = await Effect.runPromise(Deferred.make<void>());
		const scope_finalizer_started = await Effect.runPromise(Deferred.make<void>());
		const scope_finalizer_release = await Effect.runPromise(Deferred.make<void>());
		const revocation_calls: Array<string> = [];
		const connector = Layer.succeed(BrowserInspectionConnector, {
			Attach: () =>
				Effect.acquireRelease(
					Effect.succeed({
						Detach: Effect.void,
						Disconnected: Deferred.await(disconnected),
					} satisfies BrowserInspectionSession),
					() =>
						Deferred.succeed(scope_finalizer_started, undefined).pipe(
							Effect.andThen(Deferred.await(scope_finalizer_release)),
						),
				),
			Revoke: ({ connector_id, inspection_id }) =>
				Effect.gen(function* () {
					revocation_calls.push(`${connector_id}:${inspection_id}`);
					yield* Deferred.succeed(revocation_started, undefined);
					yield* Deferred.await(revocation_release);
				}),
		});
		const launcher = Layer.succeed(ExternalUrlLauncher, { Open: () => Effect.void });
		const runtime = make_runtime(database_path, launcher, connector, {
			teardown_timeout_ms: 20,
		});
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const lifecycle = yield* PreviewBrowserLifecycle;

				yield* SeedThread();
				yield* SeedTarget();
				yield* lifecycle.Attach(attach_command());
				yield* Deferred.succeed(disconnected, undefined);
				const revocation = yield* Deferred.await(revocation_started).pipe(
					Effect.timeoutOption(100),
				);
				const inspection_before_revocation = yield* lifecycle.Query({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});
				const events_before_revocation = yield* database.client
					.select()
					.from(JournalEvents);

				yield* Deferred.succeed(revocation_release, undefined);
				yield* Deferred.succeed(scope_finalizer_release, undefined);
				yield* Effect.sleep(30);
				const inspection_after_revocation = yield* lifecycle.Query({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});

				return {
					events_before_revocation,
					inspection_after_revocation,
					inspection_before_revocation,
					revocation,
					scope_finalizer: yield* Deferred.await(scope_finalizer_started).pipe(
						Effect.timeoutOption(100),
					),
				};
			}),
		);

		expect(Option.isSome(result.revocation)).toBe(true);
		expect(Option.isSome(result.scope_finalizer)).toBe(true);
		expect(revocation_calls).toEqual(["connector_1:inspection_1"]);
		expect(result.inspection_before_revocation.inspections).toMatchObject([
			{ inspection_id: "inspection_1", state: "attached" },
		]);
		expect(
			result.events_before_revocation.map((event) => JSON.parse(event.payload_json)),
		).toEqual([expect.objectContaining({ action: "attached" })]);
		expect(result.inspection_after_revocation.inspections).toMatchObject([
			{
				inspection_id: "inspection_1",
				reason: "connection_lost",
				state: "disconnected",
			},
		]);
	});

	it("keeps runtime disposal pending until the connector scope revokes authority", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const finalizer_started = await Effect.runPromise(Deferred.make<void>());
		const finalizer_release = await Effect.runPromise(Deferred.make<void>());
		const connector = Layer.succeed(BrowserInspectionConnector, {
			Revoke: () => Deferred.await(finalizer_release),
			Attach: () =>
				Effect.acquireRelease(
					Effect.succeed({
						Detach: Effect.void,
						Disconnected: Effect.never,
					} satisfies BrowserInspectionSession),
					() =>
						Effect.gen(function* () {
							yield* Deferred.succeed(finalizer_started, undefined);
							yield* Deferred.await(finalizer_release);
						}),
				),
		});
		const launcher = Layer.succeed(ExternalUrlLauncher, { Open: () => Effect.void });
		const runtime = make_runtime(database_path, launcher, connector, {
			teardown_timeout_ms: 20,
		});

		await runtime.runPromise(
			Effect.gen(function* () {
				yield* SeedThread();
				yield* SeedTarget();
				yield* PreviewBrowserLifecycle.pipe(
					Effect.flatMap((lifecycle) => lifecycle.Attach(attach_command())),
				);
			}),
		);

		runtimes.splice(runtimes.indexOf(runtime), 1);
		const disposal = runtime.dispose();

		try {
			await Effect.runPromise(Deferred.await(finalizer_started).pipe(Effect.timeout(100)));
			const completed_before_release = await Effect.runPromise(
				Effect.promise(() => disposal).pipe(Effect.timeoutOption(50)),
			);

			expect(Option.isNone(completed_before_release)).toBe(true);
		} finally {
			await Effect.runPromise(Deferred.succeed(finalizer_release, undefined));
			await disposal;
		}
	});

	it("detaches while a disconnect observer finalizer is still draining", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const observer_started = await Effect.runPromise(Deferred.make<void>());
		const finalizer_started = await Effect.runPromise(Deferred.make<void>());
		const finalizer_release = await Effect.runPromise(Deferred.make<void>());
		const finalizer_completed = await Effect.runPromise(Deferred.make<void>());
		const detach_completed = await Effect.runPromise(Deferred.make<void>());
		const scope_released = await Effect.runPromise(Deferred.make<void>());
		const connector = Layer.succeed(BrowserInspectionConnector, {
			Revoke: NoopRevoke,
			Attach: () =>
				Effect.acquireRelease(
					Effect.succeed({
						Detach: Deferred.succeed(detach_completed, undefined),
						Disconnected: Deferred.succeed(observer_started, undefined).pipe(
							Effect.andThen(Effect.never),
							Effect.ensuring(
								Effect.gen(function* () {
									yield* Deferred.succeed(finalizer_started, undefined);
									yield* Deferred.await(finalizer_release);
									yield* Deferred.succeed(finalizer_completed, undefined);
								}),
							),
						),
					} satisfies BrowserInspectionSession),
					() => Deferred.succeed(scope_released, undefined),
				),
		});
		const launcher = Layer.succeed(ExternalUrlLauncher, { Open: () => Effect.void });
		const runtime = make_runtime(database_path, launcher, connector, {
			teardown_timeout_ms: 20,
		});
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const lifecycle = yield* PreviewBrowserLifecycle;

				yield* SeedThread();
				yield* SeedTarget();
				yield* lifecycle.Attach(attach_command());
				yield* Deferred.await(observer_started).pipe(Effect.timeout(100));

				const removal = yield* lifecycle
					.SynchronizeTargetRemoval(
						{
							project_id: "project_1",
							target_id: "preview_1",
							workspace_id: "workspace_1",
						},
						() =>
							database.client
								.delete(PreviewTargets)
								.pipe(Effect.as({ status: "accepted" as const })),
					)
					.pipe(Effect.forkChild({ startImmediately: true }));

				yield* Deferred.await(finalizer_started).pipe(Effect.timeout(100));
				yield* Deferred.await(detach_completed).pipe(Effect.timeout(100));
				yield* Deferred.await(scope_released).pipe(Effect.timeout(100));

				const completed_before_release = yield* Fiber.await(removal).pipe(
					Effect.timeout(100),
				);
				const finalizer_before_release = yield* Deferred.isDone(finalizer_completed);

				yield* Deferred.succeed(finalizer_release, undefined);
				yield* Deferred.await(finalizer_completed).pipe(Effect.timeout(100));

				return { completed_before_release, finalizer_before_release };
			}),
		);

		expect(Exit.isSuccess(result.completed_before_release)).toBe(true);
		expect(result.finalizer_before_release).toBe(false);
	});

	it("rejects changed launch intent after an accepted command identity", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const launcher: LauncherControl = { calls: [], observed_states: [] };
		const connector = await make_connector();
		const runtime = make_runtime(database_path, make_launcher(launcher), connector.layer);
		const error = await runtime.runPromise(
			Effect.gen(function* () {
				const lifecycle = yield* PreviewBrowserLifecycle;

				yield* SeedThread();
				yield* SeedTarget();
				yield* SeedTarget("preview_2");
				yield* lifecycle.Open(open_command());

				return yield* lifecycle.Open(open_command("open_1", "preview_2")).pipe(Effect.flip);
			}),
		);

		expect(error).toMatchObject({ code: "conflict", subject_id: "open_1" });
		expect(launcher.calls).toHaveLength(1);
	});

	it("recovers a dispatching launch as outcome unknown without launching", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const first_launcher: LauncherControl = { calls: [], observed_states: [] };
		const first_connector = await make_connector();
		const first = make_runtime(
			database_path,
			make_launcher(first_launcher),
			first_connector.layer,
		);

		await first.runPromise(
			Effect.gen(function* () {
				const repository = yield* PreviewBrowserRepository;

				yield* SeedThread();
				yield* SeedTarget();
				yield* repository.PrepareLaunch(open_command(), 5, 1);
			}),
		);
		await first.dispose();
		runtimes.splice(runtimes.indexOf(first), 1);

		const second_launcher: LauncherControl = { calls: [], observed_states: [] };
		const second_connector = await make_connector();
		const second = make_runtime(
			database_path,
			make_launcher(second_launcher),
			second_connector.layer,
		);
		const result = await second.runPromise(
			Effect.gen(function* () {
				const lifecycle = yield* PreviewBrowserLifecycle;

				const query = yield* lifecycle.Query({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});
				const replayed = yield* lifecycle.Open(open_command());

				return { query, replayed };
			}),
		);

		expect(result.query.launches).toMatchObject([
			{ reason: "interrupted", state: "outcome_unknown" },
		]);
		expect(result.replayed.status).toBe("duplicate");
		expect(second_launcher.calls).toEqual([]);
	});

	it("recovers interrupted operations in deterministic 256-row batches", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const launcher: LauncherControl = { calls: [], observed_states: [] };
		const connector = await make_connector();
		const runtime = make_runtime(database_path, make_launcher(launcher), connector.layer, {
			recovery_interval_ms: 60_000,
		});
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const repository = yield* PreviewBrowserRepository;
				const commands = Array.from({ length: 257 }, (_, index) =>
					open_command(`open_${index.toString().padStart(3, "0")}`),
				).reverse();

				yield* SeedThread();
				yield* SeedTarget();
				yield* Effect.forEach(
					commands,
					(command) => repository.PrepareLaunch(command, 0, 1),
					{ concurrency: 1, discard: true },
				);

				const first = yield* repository.RecoverInterrupted(10_000, []);
				const second = yield* repository.RecoverInterrupted(10_000, []);
				const third = yield* repository.RecoverInterrupted(10_000, []);

				return { first, second, third };
			}),
		);

		expect(result.first).toHaveLength(256);
		expect(result.first.map((event) => event.causation_id)).toEqual(
			Array.from({ length: 256 }, (_, index) => `open_${index.toString().padStart(3, "0")}`),
		);
		expect(result.second.map((event) => event.causation_id)).toEqual(["open_256"]);
		expect(result.third).toEqual([]);
		expect(launcher.calls).toEqual([]);
	});

	it("attaches, detaches, and disconnects scoped sessions for target and thread fences", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const launcher: LauncherControl = { calls: [], observed_states: [] };
		const connector = await make_connector();
		const runtime = make_runtime(database_path, make_launcher(launcher), connector.layer);
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const lifecycle = yield* PreviewBrowserLifecycle;

				yield* SeedThread();
				yield* SeedThread("thread_2");
				yield* SeedTarget();
				yield* SeedTarget("preview_2");
				yield* lifecycle.Attach(attach_command());
				yield* lifecycle.Detach(detach_command());
				yield* lifecycle.Attach(attach_command("attach_2", "inspection_2"));
				yield* lifecycle.Attach(
					attach_command("attach_3", "inspection_3", "preview_2", "thread_2"),
				);
				yield* lifecycle.SynchronizeTargetRemoval(
					{
						project_id: "project_1",
						target_id: "preview_1",
						workspace_id: "workspace_1",
					},
					() => Effect.succeed({ status: "accepted" as const }),
				);
				yield* lifecycle.QuiesceThread("thread_2");

				return yield* lifecycle.Query({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});
			}),
		);

		expect(connector.control.attach_calls).toEqual([
			"inspection_1",
			"inspection_2",
			"inspection_3",
		]);
		expect(connector.control.detach_calls).toEqual([
			"inspection_1",
			"inspection_2",
			"inspection_3",
		]);
		expect(connector.control.scope_releases).toEqual([
			"inspection_1",
			"inspection_2",
			"inspection_3",
		]);
		expect(result.inspections).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					inspection_id: "inspection_1",
					reason: "detached",
					state: "disconnected",
				}),
				expect.objectContaining({
					inspection_id: "inspection_2",
					reason: "target_changed",
					state: "disconnected",
				}),
				expect.objectContaining({
					inspection_id: "inspection_3",
					reason: "thread_erased",
					state: "disconnected",
				}),
			]),
		);
	});

	it("interrupts a pending disconnect observer when its inspection detaches", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const launcher: LauncherControl = { calls: [], observed_states: [] };
		const connector = await make_connector();
		const runtime = make_runtime(database_path, make_launcher(launcher), connector.layer);

		await runtime.runPromise(
			Effect.gen(function* () {
				const lifecycle = yield* PreviewBrowserLifecycle;

				yield* SeedThread();
				yield* SeedTarget();
				yield* lifecycle.Attach(attach_command());

				const started = connector.control.observer_started.get("inspection_1");
				const released = connector.control.observer_released.get("inspection_1");

				if (!started || !released) {
					return yield* Effect.die("Disconnect observer fixture was not registered");
				}

				yield* Deferred.await(started).pipe(Effect.timeout(100));
				yield* lifecycle.Detach(detach_command());
				yield* Deferred.await(released).pipe(Effect.timeout(100));
			}),
		);

		expect(connector.control.detach_calls).toEqual(["inspection_1"]);
		expect(connector.control.scope_releases).toEqual(["inspection_1"]);
	});

	it("holds the lifecycle gate across target removal and fences only accepted removal", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const launcher: LauncherControl = { calls: [], observed_states: [] };
		const connector = await make_connector();
		const runtime = make_runtime(database_path, make_launcher(launcher), connector.layer);
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const lifecycle = yield* PreviewBrowserLifecycle;
				const removal_started = yield* Deferred.make<void>();
				const removal_release = yield* Deferred.make<void>();
				const target = {
					project_id: "project_1",
					target_id: "preview_1",
					workspace_id: "workspace_1",
				} as const;

				yield* SeedThread();
				yield* SeedTarget();
				yield* lifecycle.Attach(attach_command());

				const rejected = yield* lifecycle
					.SynchronizeTargetRemoval(target, () => Effect.fail("remove rejected"))
					.pipe(Effect.flip);
				const after_rejection = yield* lifecycle.Query({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});
				const removal = yield* lifecycle
					.SynchronizeTargetRemoval(target, () =>
						Effect.gen(function* () {
							yield* Deferred.succeed(removal_started, undefined);
							yield* Deferred.await(removal_release);
							yield* database.client.delete(PreviewTargets);

							return { status: "accepted" as const };
						}),
					)
					.pipe(Effect.forkChild({ startImmediately: true }));

				yield* Deferred.await(removal_started);

				const attach = yield* lifecycle
					.Attach(attach_command("attach_2", "inspection_2"))
					.pipe(Effect.forkChild({ startImmediately: true }));

				yield* Effect.yieldNow;

				const attached_before_release = attach.pollUnsafe() !== undefined;

				yield* Deferred.succeed(removal_release, undefined);
				yield* Fiber.join(removal);

				const attach_error = yield* Fiber.join(attach).pipe(Effect.flip);
				const after_removal = yield* lifecycle.Query({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});

				return {
					after_rejection,
					after_removal,
					attach_error,
					attached_before_release,
					rejected,
				};
			}),
		);

		expect(result.rejected).toBe("remove rejected");
		expect(result.after_rejection.inspections).toMatchObject([
			{ inspection_id: "inspection_1", state: "attached" },
		]);
		expect(result.attached_before_release).toBe(false);
		expect(result.attach_error).toMatchObject({ code: "not_found" });
		expect(connector.control.attach_calls).toEqual(["inspection_1"]);
		expect(connector.control.detach_calls).toEqual(["inspection_1"]);
		expect(connector.control.scope_releases).toEqual(["inspection_1"]);
		expect(result.after_removal.inspections).toMatchObject([
			{
				inspection_id: "inspection_1",
				reason: "target_changed",
				state: "disconnected",
			},
		]);
	});

	it("renews a long-running target-removal lease before removing its claimed generation", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const removal_started = await Effect.runPromise(Deferred.make<void>());
		const removal_release = await Effect.runPromise(Deferred.make<void>());
		const first_clock: ClockControl = { now_ms: 0 };
		const second_clock: ClockControl = { now_ms: 61 };
		const launcher = Layer.succeed(ExternalUrlLauncher, { Open: () => Effect.void });
		const first_connector = await make_connector();
		const second_connector = await make_connector();
		const options = {
			target_removal_lease_ms: 60,
			teardown_timeout_ms: 10,
		};
		const first = make_runtime(
			database_path,
			launcher,
			first_connector.layer,
			options,
			first_clock,
		);
		const second = make_runtime(
			database_path,
			launcher,
			second_connector.layer,
			options,
			second_clock,
		);
		const result = await first.runPromise(
			Effect.gen(function* () {
				const lifecycle = yield* PreviewBrowserLifecycle;
				const target_repository = yield* PreviewTargetRepository;

				yield* SeedThread();
				yield* SeedTarget();
				yield* lifecycle.Attach(attach_command());

				const removal = yield* lifecycle
					.SynchronizeTargetRemoval(
						{
							project_id: "project_1",
							target_id: "preview_1",
							workspace_id: "workspace_1",
						},
						(claim) =>
							Effect.gen(function* () {
								yield* Deferred.succeed(removal_started, undefined);
								yield* Deferred.await(removal_release);

								const acceptance = yield* target_repository.RemoveClaimed(
									remove_command("remove_renewed_generation"),
									claim,
									first_clock.now_ms,
								);

								return { acceptance, claim, status: acceptance.status };
							}),
					)
					.pipe(Effect.forkChild({ startImmediately: true }));

				yield* Deferred.await(removal_started);
				first_clock.now_ms = 20;
				yield* Effect.sleep(30);

				const competing_claim_error = yield* Effect.promise(() =>
					second.runPromise(
						Effect.flatMap(PreviewBrowserRepository, (repository) =>
							repository
								.ClaimTargetRemoval(
									{
										project_id: "project_1",
										target_id: "preview_1",
										workspace_id: "workspace_1",
									},
									second_clock.now_ms,
									60,
								)
								.pipe(Effect.flip),
						),
					),
				);

				first_clock.now_ms = second_clock.now_ms;
				yield* Deferred.succeed(removal_release, undefined);
				const completed = yield* Fiber.join(removal);
				const query = yield* lifecycle.Query({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});

				return { competing_claim_error, completed, query };
			}),
		);

		expect(result.competing_claim_error).toMatchObject({ reason: "target_removing" });
		expect(result.completed.claim.subject).toEqual({
			_tag: "Current",
			target_generation_id: "preview_1_generation_1",
		});
		expect(result.completed.acceptance.event.payload).toMatchObject({
			action: "removed",
			target: { state: "removed", target_id: "preview_1" },
		});
		expect(result.query.inspections).toMatchObject([
			{
				inspection_id: "inspection_1",
				reason: "target_changed",
				state: "disconnected",
			},
		]);
	});

	it("does not fence a replacement generation when an exact removal command replays", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const connector = await make_connector();
		const clock: ClockControl = { now_ms: 10_000 };
		const runtime = make_runtime(
			database_path,
			make_launcher({ calls: [], observed_states: [] }),
			connector.layer,
			{
				inspection_heartbeat_interval_ms: 10,
				live_inspection_lease_ms: 2_000,
			},
			clock,
		);
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const lifecycle = yield* PreviewBrowserLifecycle;
				const target_repository = yield* PreviewTargetRepository;
				const command = remove_command("remove_replayed_generation");
				const target = {
					project_id: "project_1",
					target_id: "preview_1",
					workspace_id: "workspace_1",
				} as const;

				yield* SeedThread();
				yield* SeedTarget();
				yield* lifecycle.Attach(
					attach_command("attach_generation_1", "inspection_generation_1"),
				);

				const accepted = yield* lifecycle.SynchronizeTargetRemoval(target, (claim) =>
					target_repository.RemoveClaimed(command, claim, 10_000),
				);
				yield* lifecycle.SettleTargetRemovalFence(command.message_id);

				yield* SeedTarget("preview_1", "preview_1_generation_2");
				yield* lifecycle.Attach(
					attach_command("attach_generation_2", "inspection_generation_2"),
				);
				const inspection_rows = yield* database.client
					.select({
						inspection_id: PreviewInspectionSessions.inspection_id,
						lease_expires_at_ms: PreviewInspectionSessions.lease_expires_at_ms,
					})
					.from(PreviewInspectionSessions);
				const initial_lease = inspection_rows.find(
					(row) => row.inspection_id === "inspection_generation_2",
				)?.lease_expires_at_ms;

				if (initial_lease === undefined) {
					return yield* Effect.die("Replay fixture did not persist generation 2");
				}

				const replay_started = yield* Deferred.make<void>();
				const replay_release = yield* Deferred.make<void>();
				const replay = yield* lifecycle
					.SynchronizeTargetRemoval(target, (claim) =>
						Effect.gen(function* () {
							const acceptance = yield* target_repository.RemoveClaimed(
								command,
								claim,
								10_000,
							);

							yield* Deferred.succeed(replay_started, undefined);
							yield* Deferred.await(replay_release);

							return acceptance;
						}),
					)
					.pipe(Effect.forkChild({ startImmediately: true }));

				yield* Deferred.await(replay_started);

				clock.now_ms = 10_010;

				const AwaitHeartbeatOutcome: Effect.Effect<"detached" | "renewed"> = Effect.suspend(
					() =>
						Effect.gen(function* () {
							const rows = yield* database.client
								.select({
									inspection_id: PreviewInspectionSessions.inspection_id,
									lease_expires_at_ms:
										PreviewInspectionSessions.lease_expires_at_ms,
								})
								.from(PreviewInspectionSessions)
								.pipe(Effect.orDie);
							const generation_2 = rows.find(
								(row) => row.inspection_id === "inspection_generation_2",
							);

							if (
								generation_2 !== undefined &&
								generation_2.lease_expires_at_ms > initial_lease
							) {
								return "renewed" as const;
							}

							if (
								connector.control.detach_calls.includes("inspection_generation_2")
							) {
								return "detached" as const;
							}

							yield* Effect.sleep(5);

							return yield* AwaitHeartbeatOutcome;
						}),
				);
				const heartbeat_outcome = yield* AwaitHeartbeatOutcome.pipe(Effect.timeout(500));

				yield* Deferred.succeed(replay_release, undefined);

				const replayed = yield* Fiber.join(replay);
				const query = yield* lifecycle.Query({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});

				return { accepted, heartbeat_outcome, query, replayed };
			}),
		);

		expect(result.accepted.status).toBe("accepted");
		expect(result.heartbeat_outcome).toBe("renewed");
		expect(result.replayed.status).toBe("duplicate");
		expect(connector.control.detach_calls).toEqual(["inspection_generation_1"]);
		expect(result.query.inspections).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					inspection_id: "inspection_generation_2",
					state: "attached",
				}),
			]),
		);
	});

	it("recovers an interrupted target-removal fence without touching a replacement generation", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const first_clock: ClockControl = { now_ms: 0 };
		const second_clock: ClockControl = { now_ms: 600_001 };
		const first_connector = await make_connector();
		const second_connector = await make_connector();
		const launcher = Layer.succeed(ExternalUrlLauncher, { Open: () => Effect.void });
		const options = {
			inspection_heartbeat_interval_ms: 60_000,
			live_inspection_lease_ms: 600_000,
			recovery_interval_ms: 60_000,
			target_removal_lease_ms: 50,
			teardown_timeout_ms: 10,
		};
		const first = make_runtime(
			database_path,
			launcher,
			first_connector.layer,
			options,
			first_clock,
		);
		const removal_command = remove_command("remove_interrupted_generation_1");

		await first.runPromise(
			Effect.gen(function* () {
				const lifecycle = yield* PreviewBrowserLifecycle;
				const browser_repository = yield* PreviewBrowserRepository;
				const target_repository = yield* PreviewTargetRepository;

				yield* SeedThread();
				yield* SeedTarget();
				yield* lifecycle.Attach(
					attach_command("attach_generation_1", "inspection_generation_1"),
				);

				first_clock.now_ms = 1;
				const removal_claim = yield* browser_repository.ClaimTargetRemoval(
					{
						project_id: "project_1",
						target_id: "preview_1",
						workspace_id: "workspace_1",
					},
					first_clock.now_ms,
					50,
				);
				yield* target_repository.RemoveClaimed(
					removal_command,
					removal_claim,
					first_clock.now_ms,
				);
				yield* browser_repository.ReleaseTargetRemoval(removal_claim);

				first_clock.now_ms = 600_000;
				yield* target_repository.Register(
					register_command("register_generation_2"),
					"http://localhost:5173/preview_1",
					first_clock.now_ms,
				);
				yield* lifecycle.Attach(
					attach_command("attach_generation_2", "inspection_generation_2"),
				);
			}),
		);

		const second = make_runtime(
			database_path,
			launcher,
			second_connector.layer,
			options,
			second_clock,
		);
		const result = await second.runPromise(
			Effect.gen(function* () {
				const browser_repository = yield* PreviewBrowserRepository;
				const lifecycle = yield* PreviewBrowserLifecycle;
				const target_repository = yield* PreviewTargetRepository;
				const fences = yield* browser_repository.ListTargetRemovalFences();
				const query = yield* lifecycle.Query({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});
				const replay = yield* target_repository.ReplayTargetRemoval(removal_command);

				return { fences, query, replay };
			}),
		);

		expect(first_connector.control.attach_calls).toEqual([
			"inspection_generation_1",
			"inspection_generation_2",
		]);
		expect(second_connector.control.attach_calls).toEqual([]);
		expect(result.query.inspections).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					inspection_id: "inspection_generation_1",
					reason: "target_changed",
					state: "disconnected",
				}),
				expect.objectContaining({
					inspection_id: "inspection_generation_2",
					state: "attached",
				}),
			]),
		);
		expect(result.fences).toEqual([]);
		expect(result.replay).toMatchObject({
			value: {
				fence_status: "complete",
			},
		});
	});

	it("does not rewrite a terminal target disconnect with a later detach command", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const connector = await make_connector();
		const runtime = make_runtime(
			database_path,
			make_launcher({ calls: [], observed_states: [] }),
			connector.layer,
		);
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const lifecycle = yield* PreviewBrowserLifecycle;

				yield* SeedThread();
				yield* SeedTarget();
				yield* lifecycle.Attach(attach_command());
				yield* lifecycle.SynchronizeTargetRemoval(
					{
						project_id: "project_1",
						target_id: "preview_1",
						workspace_id: "workspace_1",
					},
					() =>
						database.client
							.delete(PreviewTargets)
							.pipe(Effect.as({ status: "accepted" as const })),
				);

				const detach_error = yield* lifecycle.Detach(detach_command()).pipe(Effect.flip);
				const query = yield* lifecycle.Query({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});

				return { detach_error, query };
			}),
		);

		expect(result.detach_error).toMatchObject({
			code: "conflict",
			subject_id: "inspection_1",
		});
		expect(result.query.inspections).toMatchObject([
			{
				inspection_id: "inspection_1",
				reason: "target_changed",
				state: "disconnected",
			},
		]);
	});

	it("disconnects an attached inspection after the thread erasure claim is durable", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const launcher: LauncherControl = { calls: [], observed_states: [] };
		const connector = await make_connector();
		const runtime = make_runtime(database_path, make_launcher(launcher), connector.layer);
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const lifecycle = yield* PreviewBrowserLifecycle;

				yield* SeedThread();
				yield* SeedTarget();
				yield* lifecycle.Attach(attach_command());
				yield* database.client.insert(ThreadErasureClaims).values({
					claimed_at: now,
					thread_id: "thread_1",
				});
				yield* lifecycle.QuiesceThread("thread_1");

				return yield* lifecycle.Query({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});
			}),
		);

		expect(connector.control.detach_calls).toEqual(["inspection_1"]);
		expect(connector.control.scope_releases).toEqual(["inspection_1"]);
		expect(result.inspections).toMatchObject([
			{
				inspection_id: "inspection_1",
				reason: "thread_erased",
				state: "disconnected",
			},
		]);
	});

	it("recovers attaching and attached inspections as disconnected without reconnecting", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const first_launcher: LauncherControl = { calls: [], observed_states: [] };
		const first_connector = await make_connector();
		const first = make_runtime(
			database_path,
			make_launcher(first_launcher),
			first_connector.layer,
		);

		await first.runPromise(
			Effect.gen(function* () {
				const repository = yield* PreviewBrowserRepository;
				const attached = attach_command("attach_attached", "inspection_attached");

				yield* SeedThread();
				yield* SeedTarget();
				yield* repository.PrepareInspection(
					attach_command("attach_attaching", "inspection_attaching"),
					5,
					1,
				);
				const preparation = yield* repository.PrepareInspection(attached, 6, 1);

				if (preparation._tag !== "Prepared") {
					return yield* Effect.die("Attached recovery fixture was not prepared");
				}

				yield* repository.SettleInspectionAttach(
					attached,
					preparation.prepared.claim,
					{ state: "attached" },
					6,
				);
			}),
		);
		await first.dispose();
		runtimes.splice(runtimes.indexOf(first), 1);

		const second_launcher: LauncherControl = { calls: [], observed_states: [] };
		const second_connector = await make_connector();
		const second = make_runtime(
			database_path,
			make_launcher(second_launcher),
			second_connector.layer,
		);
		const query = await second.runPromise(
			Effect.flatMap(PreviewBrowserLifecycle, (lifecycle) =>
				lifecycle.Query({ project_id: "project_1", workspace_id: "workspace_1" }),
			),
		);

		expect(second_connector.control.attach_calls).toEqual([]);
		expect(query.inspections).toMatchObject([
			{ inspection_id: "inspection_attached", reason: "interrupted", state: "disconnected" },
			{ inspection_id: "inspection_attaching", reason: "interrupted", state: "disconnected" },
		]);
	});

	it("keeps an expired attach disconnected when its stale owner settles late", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const launcher: LauncherControl = { calls: [], observed_states: [] };
		const connector = await make_connector();
		const runtime = make_runtime(database_path, make_launcher(launcher), connector.layer, {
			recovery_interval_ms: 60_000,
		});
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const repository = yield* PreviewBrowserRepository;
				const command = attach_command("attach_late", "inspection_late");

				yield* SeedThread();
				yield* SeedTarget();

				const preparation = yield* repository.PrepareInspection(command, 5, 1);

				if (preparation._tag !== "Prepared") {
					return yield* Effect.die("Late attach fixture was not prepared");
				}

				yield* repository.RecoverInterrupted(10_000, [command.payload.inspection_id]);

				const late = yield* repository.SettleInspectionAttach(
					command,
					preparation.prepared.claim,
					{ state: "attached" },
					10_001,
				);
				const query = yield* repository.List({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});

				return { late, query };
			}),
		);

		expect(result.late.status).toBe("duplicate");
		expect(result.late.event.payload).toMatchObject({
			action: "disconnected",
			inspection: {
				inspection_id: "inspection_late",
				reason: "interrupted",
				state: "disconnected",
			},
			type: "preview.inspection.updated",
		});
		expect(result.query.inspections).toMatchObject([
			{
				inspection_id: "inspection_late",
				reason: "interrupted",
				state: "disconnected",
			},
		]);
	});

	it("completes and replays an attach detached before its late settlement", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const launcher: LauncherControl = { calls: [], observed_states: [] };
		const connector = await make_connector();
		const runtime = make_runtime(database_path, make_launcher(launcher), connector.layer);
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const lifecycle = yield* PreviewBrowserLifecycle;
				const repository = yield* PreviewBrowserRepository;
				const command = attach_command("attach_detached_late", "inspection_detached_late");

				yield* SeedThread();
				yield* SeedTarget();

				const preparation = yield* repository.PrepareInspection(command, 0, 10);

				if (preparation._tag !== "Prepared") {
					return yield* Effect.die("Detach-before-settlement fixture was not prepared");
				}

				yield* lifecycle.Detach(detach_command("inspection_detached_late"));
				const late = yield* repository.SettleInspectionAttach(
					command,
					preparation.prepared.claim,
					{ state: "attached" },
					10,
				);
				const replayed = yield* lifecycle.Attach(command);
				const command_rows = yield* database.client.select().from(JournalCommands);
				const inspection = yield* repository.List({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});

				return { command_rows, inspection, late, replayed };
			}),
		);

		const attach_row = result.command_rows.find(
			(row) => row.message_id === "attach_detached_late",
		);

		expect(attach_row?.status).toBe("accepted");
		expect(result.late.event.payload).toMatchObject({
			action: "disconnected",
			inspection: {
				inspection_id: "inspection_detached_late",
				reason: "detached",
				state: "disconnected",
			},
		});
		expect(result.replayed).toEqual({ event: result.late.event, status: "duplicate" });
		expect(result.inspection.inspections).toMatchObject([
			{
				inspection_id: "inspection_detached_late",
				reason: "detached",
				state: "disconnected",
			},
		]);
	});

	it("completes an attaching command when target fencing disconnects it", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const launcher: LauncherControl = { calls: [], observed_states: [] };
		const connector = await make_connector();
		const runtime = make_runtime(database_path, make_launcher(launcher), connector.layer);
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const repository = yield* PreviewBrowserRepository;
				const command = attach_command("attach_target_fenced", "inspection_target_fenced");

				yield* SeedThread();
				yield* SeedTarget();

				const preparation = yield* repository.PrepareInspection(command, 0, 10);

				if (preparation._tag !== "Prepared") {
					return yield* Effect.die("Target-fenced attach fixture was not prepared");
				}

				const removal_claim = yield* repository.ClaimTargetRemoval(
					{
						project_id: "project_1",
						target_id: "preview_1",
						workspace_id: "workspace_1",
					},
					1,
					10,
				);
				const disconnected = yield* repository.DisconnectTargetInspection(
					command.payload.inspection_id,
					removal_claim,
					1,
				);

				yield* repository.ReleaseTargetRemoval(removal_claim);
				const replayed = yield* repository.Replay(command);
				const commands = yield* database.client.select().from(JournalCommands);

				return { commands, disconnected, replayed };
			}),
		);

		expect(result.commands).toMatchObject([
			{ message_id: "attach_target_fenced", status: "accepted" },
		]);
		expect(result.disconnected).toMatchObject({
			value: {
				payload: {
					action: "disconnected",
					inspection: {
						reason: "target_changed",
						state: "disconnected",
					},
				},
			},
		});
		expect(result.replayed).toMatchObject({
			value: {
				event: {
					payload: {
						action: "disconnected",
					},
				},
				status: "duplicate",
			},
		});
	});

	it("fences stale inspection and launch owners after target removal claims", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const first_connector = await make_connector();
		const second_connector = await make_connector();
		const first = make_runtime(
			database_path,
			make_launcher({ calls: [], observed_states: [] }),
			first_connector.layer,
		);
		const second = make_runtime(
			database_path,
			make_launcher({ calls: [], observed_states: [] }),
			second_connector.layer,
		);
		const result = await first.runPromise(
			Effect.gen(function* () {
				const first_repository = yield* PreviewBrowserRepository;

				yield* SeedThread();
				yield* SeedTarget();

				const inspection_command = attach_command(
					"attach_expired_owner",
					"inspection_expired_owner",
				);
				const inspection_preparation = yield* first_repository.PrepareInspection(
					inspection_command,
					10_000,
					10,
				);

				if (inspection_preparation._tag !== "Prepared") {
					return yield* Effect.die("Expired inspection fixture was not prepared");
				}

				const launch_command = open_command("open_expired_owner");
				const launch_preparation = yield* first_repository.PrepareLaunch(
					launch_command,
					10_000,
					10,
				);

				if (launch_preparation._tag !== "Prepared") {
					return yield* Effect.die("Expired launch fixture was not prepared");
				}

				const removal = yield* Effect.promise(() =>
					second.runPromise(
						Effect.gen(function* () {
							const second_repository = yield* PreviewBrowserRepository;

							return yield* second_repository.ClaimTargetRemoval(
								{
									project_id: "project_1",
									target_id: "preview_1",
									workspace_id: "workspace_1",
								},
								10_011,
								10,
							);
						}),
					),
				);

				const stale_renewal = yield* first_repository
					.RenewInspectionLease(
						inspection_command.payload.inspection_id,
						inspection_preparation.prepared.claim,
						10_012,
						10,
					)
					.pipe(Effect.flip);
				const stale_inspection_settlement = yield* first_repository
					.SettleInspectionAttach(
						inspection_command,
						inspection_preparation.prepared.claim,
						{ state: "attached" },
						10_012,
					)
					.pipe(Effect.flip);
				const stale_launch_settlement = yield* first_repository
					.SettleLaunch(
						launch_command,
						launch_preparation.prepared.claim,
						{ state: "dispatched" },
						10_012,
					)
					.pipe(Effect.flip);

				return {
					removal,
					stale_inspection_settlement,
					stale_launch_settlement,
					stale_renewal,
				};
			}),
		);

		expect(result.removal.target_id).toBe("preview_1");
		expect(result.stale_renewal).toMatchObject({ reason: "ownership_lost" });
		expect(result.stale_inspection_settlement).toMatchObject({
			reason: "target_removing",
		});
		expect(result.stale_launch_settlement).toMatchObject({ reason: "target_removing" });
	});

	it("keeps a replacement target generation outside an expired removal owner's scope", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const first_connector = await make_connector();
		const second_connector = await make_connector();
		const launcher = Layer.succeed(ExternalUrlLauncher, { Open: () => Effect.void });
		const first = make_runtime(database_path, launcher, first_connector.layer);
		const second = make_runtime(database_path, launcher, second_connector.layer);
		const result = await first.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const first_repository = yield* PreviewBrowserRepository;
				const old_attach = attach_command("attach_generation_1", "inspection_generation_1");

				yield* SeedThread();
				yield* SeedTarget();
				const old_preparation = yield* first_repository.PrepareInspection(
					old_attach,
					0,
					10,
				);

				if (old_preparation._tag !== "Prepared") {
					return yield* Effect.die("Old-generation inspection was not prepared");
				}

				yield* first_repository.SettleInspectionAttach(
					old_attach,
					old_preparation.prepared.claim,
					{ state: "attached" },
					1,
				);
				const expired_claim = yield* first_repository.ClaimTargetRemoval(
					{
						project_id: "project_1",
						target_id: "preview_1",
						workspace_id: "workspace_1",
					},
					10,
					10,
				);
				const replacement = yield* Effect.promise(() =>
					second.runPromise(
						Effect.gen(function* () {
							const database = yield* Database;
							const repository = yield* PreviewBrowserRepository;
							const target_repository = yield* PreviewTargetRepository;

							const claim = yield* repository.ClaimTargetRemoval(
								{
									project_id: "project_1",
									target_id: "preview_1",
									workspace_id: "workspace_1",
								},
								20,
								10,
							);

							yield* repository.DisconnectTargetInspection(
								"inspection_generation_1",
								claim,
								20,
							);
							yield* target_repository.RemoveClaimed(
								remove_command("remove_generation_1"),
								claim,
								20,
							);
							yield* repository.ReleaseTargetRemoval(claim);
							yield* SeedTarget("preview_1", "preview_1_generation_2");
							const replacement_attach = attach_command(
								"attach_generation_2",
								"inspection_generation_2",
							);
							const prepared = yield* repository.PrepareInspection(
								replacement_attach,
								21,
								100,
							);

							if (prepared._tag !== "Prepared") {
								return yield* Effect.die("Replacement inspection was not prepared");
							}

							yield* repository.SettleInspectionAttach(
								replacement_attach,
								prepared.prepared.claim,
								{ state: "attached" },
								21,
							);

							return yield* database.client.select().from(PreviewTargets);
						}),
					),
				);
				const target_repository = yield* PreviewTargetRepository;
				const stale_remove = yield* target_repository
					.RemoveClaimed(remove_command("remove_stale_generation"), expired_claim, 21)
					.pipe(Effect.flip);
				const stale_discovery = yield* first_repository
					.ActiveInspectionIdsForTargetRemoval(expired_claim, 21)
					.pipe(Effect.flip);
				const stale_disconnect = yield* first_repository
					.DisconnectTargetInspection("inspection_generation_2", expired_claim, 21)
					.pipe(Effect.flip);
				const targets = yield* database.client.select().from(PreviewTargets);
				const lifecycle = yield* first_repository.List({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});

				return {
					lifecycle,
					replacement,
					stale_disconnect,
					stale_discovery,
					stale_remove,
					targets,
				};
			}),
		);

		expect(result.replacement).toMatchObject([
			{ generation_id: "preview_1_generation_2", target_id: "preview_1" },
		]);
		expect(result.stale_remove).toMatchObject({ reason: "target_removing" });
		expect(result.stale_discovery).toMatchObject({ reason: "ownership_lost" });
		expect(result.stale_disconnect).toMatchObject({ reason: "ownership_lost" });
		expect(result.targets).toMatchObject([
			{ generation_id: "preview_1_generation_2", target_id: "preview_1" },
		]);
		expect(result.lifecycle.inspections).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					inspection_id: "inspection_generation_2",
					state: "attached",
				}),
			]),
		);
	});

	it("does not let an expired disconnect observer settle through target removal or replacement", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const first_connector = await make_connector();
		const second_connector = await make_connector();
		const launcher = Layer.succeed(ExternalUrlLauncher, { Open: () => Effect.void });
		const second_clock: ClockControl = { now_ms: 10 };
		const first = make_runtime(database_path, launcher, first_connector.layer);
		const second = make_runtime(
			database_path,
			launcher,
			second_connector.layer,
			{},
			second_clock,
		);
		const result = await first.runPromise(
			Effect.gen(function* () {
				const first_repository = yield* PreviewBrowserRepository;
				const command = attach_command(
					"attach_stale_observer",
					"inspection_stale_observer",
				);

				yield* SeedThread();
				yield* SeedTarget();
				const preparation = yield* first_repository.PrepareInspection(command, 10, 10);

				if (preparation._tag !== "Prepared") {
					return yield* Effect.die("Stale observer fixture was not prepared");
				}

				yield* first_repository.SettleInspectionAttach(
					command,
					preparation.prepared.claim,
					{ state: "attached" },
					10,
				);
				const replacement = yield* Effect.promise(() =>
					second.runPromise(
						Effect.gen(function* () {
							const database = yield* Database;
							const repository = yield* PreviewBrowserRepository;

							const claim = yield* repository.ClaimTargetRemoval(
								{
									project_id: "project_1",
									target_id: "preview_1",
									workspace_id: "workspace_1",
								},
								20,
								10,
							);

							yield* database.client.delete(PreviewTargets);
							yield* SeedTarget("preview_1", "preview_1_generation_2");

							return claim;
						}),
					),
				);
				const stale_disconnect = yield* first_repository.DisconnectOwnedInspection(
					"inspection_stale_observer",
					preparation.prepared.claim,
					"connection_lost",
					21,
				);
				const query = yield* first_repository.List({
					project_id: "project_1",
					workspace_id: "workspace_1",
				});

				return { query, replacement, stale_disconnect };
			}),
		);

		expect(result.replacement.claim_token).not.toBeUndefined();
		expect(Option.isNone(result.stale_disconnect)).toBe(true);
		expect(result.query.inspections).toMatchObject([
			{ inspection_id: "inspection_stale_observer", state: "attached" },
		]);
	});

	it("cleans up an observed disconnect when durable disconnect recording rejects corrupt state", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const disconnected = await Effect.runPromise(Deferred.make<void>());
		const observer_started = await Effect.runPromise(Deferred.make<void>());
		const scope_released = await Effect.runPromise(Deferred.make<void>());
		const teardown_completed = await Effect.runPromise(Deferred.make<void>());
		const connector = Layer.succeed(BrowserInspectionConnector, {
			Revoke: NoopRevoke,
			Attach: () =>
				Effect.acquireRelease(
					Effect.succeed({
						Detach: Deferred.succeed(teardown_completed, undefined),
						Disconnected: Deferred.succeed(observer_started, undefined).pipe(
							Effect.andThen(Deferred.await(disconnected)),
						),
					} satisfies BrowserInspectionSession),
					() => Deferred.succeed(scope_released, undefined),
				),
		});
		const launcher = Layer.succeed(ExternalUrlLauncher, { Open: () => Effect.void });
		const runtime = make_runtime(database_path, launcher, connector, {
			inspection_heartbeat_interval_ms: 60_000,
			live_inspection_lease_ms: 600_000,
		});
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const lifecycle = yield* PreviewBrowserLifecycle;

				yield* SeedThread();
				yield* SeedTarget();
				yield* lifecycle.Attach(attach_command());
				yield* Deferred.await(observer_started);
				yield* database.client
					.update(PreviewInspectionSessions)
					.set({ attach_command_json: "{corrupt" });
				yield* Deferred.succeed(disconnected, undefined);
				yield* Deferred.await(teardown_completed).pipe(Effect.timeout(100));
				yield* Deferred.await(scope_released).pipe(Effect.timeout(100));
				const events = yield* database.client.select().from(JournalEvents);
				const inspections = yield* database.client.select().from(PreviewInspectionSessions);

				return { events, inspections };
			}),
		);

		expect(result.events.map((event) => JSON.parse(event.payload_json))).toEqual([
			expect.objectContaining({ action: "attached" }),
		]);
		expect(result.inspections).toMatchObject([
			{ inspection_id: "inspection_1", reason: null, state: "attached" },
		]);
	});

	it("tears down a live inspection when its target disappears after removal", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const launcher: LauncherControl = { calls: [], observed_states: [] };
		const connector = await make_connector();
		const runtime = make_runtime(database_path, make_launcher(launcher), connector.layer, {
			inspection_heartbeat_interval_ms: 10,
		});
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const lifecycle = yield* PreviewBrowserLifecycle;

				yield* SeedThread();
				yield* SeedTarget();
				yield* lifecycle.Attach(attach_command());

				const detach_completed = connector.control.detach_completed.get("inspection_1");

				if (!detach_completed) {
					return yield* Effect.die("Target-change fixture did not register teardown");
				}

				yield* database.client.delete(PreviewTargets);

				const AwaitTargetChanged: Effect.Effect<
					PreviewBrowserLifecycleQueryResult,
					PreviewBrowserLifecycleError
				> = Effect.suspend(() =>
					lifecycle
						.Query({ project_id: "project_1", workspace_id: "workspace_1" })
						.pipe(
							Effect.flatMap((query) =>
								query.inspections[0]?.reason === "target_changed"
									? Effect.succeed(query)
									: Effect.sleep(5).pipe(Effect.andThen(AwaitTargetChanged)),
							),
						),
				);

				const query = yield* AwaitTargetChanged.pipe(Effect.timeout(500));

				yield* Deferred.await(detach_completed).pipe(Effect.timeout(100));

				return query;
			}),
		);

		expect(connector.control.detach_calls).toEqual(["inspection_1"]);
		expect(connector.control.scope_releases).toEqual(["inspection_1"]);
		expect(result.inspections).toMatchObject([
			{
				inspection_id: "inspection_1",
				reason: "target_changed",
				state: "disconnected",
			},
		]);
	});

	it("persists a disconnected event when an attach command uses the legacy disconnect id", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const launcher: LauncherControl = { calls: [], observed_states: [] };
		const connector = await make_connector();
		const runtime = make_runtime(database_path, make_launcher(launcher), connector.layer);
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const repository = yield* PreviewBrowserRepository;
				const command = attach_command(
					"preview_inspection_disconnected_inspection_collision",
					"inspection_collision",
				);

				yield* SeedThread();
				yield* SeedTarget();

				const preparation = yield* repository.PrepareInspection(command, 0, 10);

				if (preparation._tag !== "Prepared") {
					return yield* Effect.die("Disconnect-id collision fixture was not prepared");
				}

				yield* repository.SettleInspectionAttach(
					command,
					preparation.prepared.claim,
					{ state: "attached" },
					1,
				);
				const disconnected = yield* repository.DisconnectOwnedInspection(
					"inspection_collision",
					preparation.prepared.claim,
					"connection_lost",
					2,
				);
				const replayed = yield* repository.Replay(command);
				const events = yield* database.client.select().from(JournalEvents);

				return { disconnected, events, replayed };
			}),
		);

		expect(result.disconnected).toMatchObject({
			value: {
				payload: {
					action: "disconnected",
					inspection: {
						inspection_id: "inspection_collision",
						state: "disconnected",
					},
				},
			},
		});
		expect(result.events).toHaveLength(2);
		expect(result.events.map((event) => JSON.parse(event.payload_json))).toEqual(
			expect.arrayContaining([
				expect.objectContaining({ action: "attached" }),
				expect.objectContaining({ action: "disconnected" }),
			]),
		);
		expect(result.replayed).toMatchObject({
			value: {
				event: {
					payload: {
						action: "attached",
					},
				},
			},
		});
	});

	it("returns source-safe lifecycle projections and serialized journal records", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const launcher: LauncherControl = { calls: [], observed_states: [] };
		const connector = await make_connector();
		const runtime = make_runtime(database_path, make_launcher(launcher), connector.layer);
		const result = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const lifecycle = yield* PreviewBrowserLifecycle;

				yield* SeedThread();
				yield* SeedTarget();
				yield* lifecycle.Open(open_command());
				yield* lifecycle.Attach(attach_command());

				return {
					commands: yield* database.client.select().from(JournalCommands),
					events: yield* database.client.select().from(JournalEvents),
					inspections: yield* database.client.select().from(PreviewInspectionSessions),
					launches: yield* database.client.select().from(PreviewBrowserLaunches),
					query: yield* lifecycle.Query({
						project_id: "project_1",
						workspace_id: "workspace_1",
					}),
				};
			}),
		);

		expect(has_forbidden_key(result.query)).toBe(false);
		expect(
			has_forbidden_key(
				decode_json_records([
					...result.commands.flatMap((command) => [
						command.payload_json,
						command.raw_origin_json,
					]),
					...result.events.flatMap((event) => [
						event.payload_json,
						event.raw_origin_json,
					]),
					...result.launches.map((launch) => launch.command_json),
					...result.inspections.map((inspection) => inspection.attach_command_json),
				]),
			),
		).toBe(false);
		expect(JSON.stringify(result)).not.toContain("secret-browser-material");
	});
});
