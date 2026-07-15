import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	BrowserInspectionConnector,
	ExternalUrlLauncher,
	make_backend_runtime,
	ProtocolRouter,
	ProtocolServer,
} from "@artisan/backend";
import { TransportRuntime } from "@artisan/transport";

import { Database } from "../../modules/backend/src/persistence/database";
import {
	JournalCommands,
	JournalEvents,
	PreviewBrowserLaunches,
	PreviewInspectionSessions,
} from "../../modules/backend/src/persistence/schema";
import {
	make_transport_test_harness_with_protocol_server,
	wait_for,
} from "./message-channel-harness";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const now = "2026-07-15T14:00:00.000Z";

const MakeDatabasePath = Effect.flatMap(FileSystem.FileSystem, (file_system) =>
	file_system.makeTempDirectory({ prefix: "artisan-preview-browser-transport-" }).pipe(
		Effect.tap((path) => Effect.sync(() => temporary_directories.push(path))),
		Effect.map((path) => `${path}/artisan.db`),
	),
);

function make_launcher_layer(state: { calls: Array<string> }) {
	return Layer.succeed(ExternalUrlLauncher, {
		Open: (url) => Effect.sync(() => state.calls.push(url)).pipe(Effect.asVoid),
	});
}

function make_transport_runtime_layer() {
	let next_id = 0;

	return Layer.succeed(TransportRuntime, {
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.succeed(now),
	});
}

async function start_stack(
	database_path: string,
	launcher: Layer.Layer<ExternalUrlLauncher>,
	options: {
		readonly browser_inspection_connector?: Layer.Layer<BrowserInspectionConnector>;
		readonly drop_first_command_receipt?: boolean;
		readonly drop_first_preview_browser_lifecycle_result?: boolean;
		readonly initialize?: boolean;
		readonly transport_runtime: Layer.Layer<TransportRuntime>;
	},
) {
	const runtime = make_backend_runtime({
		...(options.browser_inspection_connector === undefined
			? {}
			: { browser_inspection_connector: options.browser_inspection_connector }),
		database_path,
		external_url_launcher: launcher,
		migrations_path,
	});
	const protocol_server = await runtime.runPromise(ProtocolServer);

	if (options.initialize) {
		const router = await runtime.runPromise(ProtocolRouter);

		await runtime.runPromise(
			router.Route({
				kind: "command",
				message_id: "preview_browser_thread_create",
				origin: "frontend",
				payload: { title: "External browser transport", type: "thread.create" },
				protocol_version: 1,
				schema_version: 1,
				sent_at: now,
				thread_id: "thread_preview_browser",
			}),
		);
		await runtime.runPromise(
			router.Route({
				kind: "command",
				message_id: "preview_browser_target_register",
				origin: "frontend",
				payload: {
					project_id: "project_preview_browser",
					target_id: "target_preview_browser",
					type: "preview.target.register",
					url: "http://127.0.0.1:4173/app",
					workspace_id: "workspace_preview_browser",
				},
				protocol_version: 1,
				schema_version: 1,
				sent_at: now,
				thread_id: "thread_preview_browser",
			}),
		);
	}

	const harness = await make_transport_test_harness_with_protocol_server(protocol_server, {
		client: { reconnect_delay_ms: 5 },
		...(options.drop_first_command_receipt === undefined
			? {}
			: { drop_first_command_receipt: options.drop_first_command_receipt }),
		...(options.drop_first_preview_browser_lifecycle_result === undefined
			? {}
			: {
					drop_first_preview_browser_lifecycle_result:
						options.drop_first_preview_browser_lifecycle_result,
				}),
		transport_runtime: options.transport_runtime,
	});

	return { harness, runtime };
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

describe("ArtisanClient external-browser lifecycle with the backend ProtocolServer", () => {
	it("attaches and detaches an explicit inspection through the typed client", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const launcher_state = { calls: [] as Array<string> };
		const connector_state = {
			attach_calls: [] as Array<string>,
			detach_calls: [] as Array<string>,
			scope_releases: [] as Array<string>,
		};
		const connector = Layer.succeed(BrowserInspectionConnector, {
			Attach: ({ inspection_id }) =>
				Effect.acquireRelease(
					Effect.sync(() => {
						connector_state.attach_calls.push(inspection_id);

						return {
							Detach: Effect.sync(() =>
								connector_state.detach_calls.push(inspection_id),
							),
							Disconnected: Effect.never,
						};
					}),
					() => Effect.sync(() => connector_state.scope_releases.push(inspection_id)),
				),
			Revoke: () => Effect.void,
		});
		const stack = await start_stack(database_path, make_launcher_layer(launcher_state), {
			browser_inspection_connector: connector,
			initialize: true,
			transport_runtime: make_transport_runtime_layer(),
		});

		try {
			const attached = await Effect.runPromise(
				stack.harness.client.AttachPreviewInspection({
					command_id: "preview_inspection_attach",
					connector_id: "connector_explicit",
					inspection_id: "inspection_explicit",
					project_id: "project_preview_browser",
					target_id: "target_preview_browser",
					thread_id: "thread_preview_browser",
					workspace_id: "workspace_preview_browser",
				}),
			);
			const visible_attached = await Effect.runPromise(
				stack.harness.client.GetPreviewBrowserLifecycle({
					project_id: "project_preview_browser",
					workspace_id: "workspace_preview_browser",
				}),
			);
			const detached = await Effect.runPromise(
				stack.harness.client.DetachPreviewInspection({
					command_id: "preview_inspection_detach",
					inspection_id: "inspection_explicit",
					project_id: "project_preview_browser",
					thread_id: "thread_preview_browser",
					workspace_id: "workspace_preview_browser",
				}),
			);
			const visible_detached = await Effect.runPromise(
				stack.harness.client.GetPreviewBrowserLifecycle({
					project_id: "project_preview_browser",
					workspace_id: "workspace_preview_browser",
				}),
			);
			const rows = await stack.runtime.runPromise(
				Effect.flatMap(Database, (database) =>
					database.client.select().from(PreviewInspectionSessions),
				),
			);

			expect(attached.status).toBe("accepted");
			expect(detached.status).toBe("accepted");
			expect(connector_state.attach_calls).toEqual(["inspection_explicit"]);
			expect(connector_state.detach_calls).toEqual(["inspection_explicit"]);
			expect(connector_state.scope_releases).toEqual(["inspection_explicit"]);
			expect(visible_attached.inspections).toMatchObject([
				{ inspection_id: "inspection_explicit", state: "attached" },
			]);
			expect(visible_detached.inspections).toMatchObject([
				{
					inspection_id: "inspection_explicit",
					reason: "detached",
					state: "disconnected",
				},
			]);
			expect(rows).toMatchObject([
				{
					connector_id: "connector_explicit",
					inspection_id: "inspection_explicit",
					reason: "detached",
					state: "disconnected",
				},
			]);
		} finally {
			await stack.harness.dispose();
			await stack.runtime.dispose();
		}
	});

	it("replays exact opens and queries across reconnect and restart without a second handoff", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const launcher_state = { calls: [] as Array<string> };
		const launcher = make_launcher_layer(launcher_state);
		const transport_runtime = make_transport_runtime_layer();
		const open_input = {
			command_id: "preview_browser_open_reconnect",
			project_id: "project_preview_browser",
			target_id: "target_preview_browser",
			thread_id: "thread_preview_browser",
			workspace_id: "workspace_preview_browser",
		};
		let first: Awaited<ReturnType<typeof start_stack>> | undefined = await start_stack(
			database_path,
			launcher,
			{
				drop_first_command_receipt: true,
				drop_first_preview_browser_lifecycle_result: true,
				initialize: true,
				transport_runtime,
			},
		);
		let second: Awaited<ReturnType<typeof start_stack>> | undefined;

		try {
			const receipt = await Effect.runPromise(
				first.harness.client.OpenPreviewInExternalBrowser(open_input),
			);

			await wait_for(() => first!.harness.connector_snapshot().connections === 2);

			const lifecycle = await Effect.runPromise(
				first.harness.client.GetPreviewBrowserLifecycle({
					project_id: "project_preview_browser",
					workspace_id: "workspace_preview_browser",
				}),
			);

			await wait_for(
				() =>
					first!.harness.connector_snapshot().preview_browser_lifecycle_query_attempts
						.length === 2,
			);

			const connector = first.harness.connector_snapshot();
			const rows = await first.runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					return {
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						launches: yield* database.client.select().from(PreviewBrowserLaunches),
					};
				}),
			);

			expect(receipt.status).toBe("duplicate");
			expect(launcher_state.calls).toEqual(["http://127.0.0.1:4173/app"]);
			expect(connector.preview_browser_lifecycle_query_attempts[1]).toEqual(
				connector.preview_browser_lifecycle_query_attempts[0],
			);
			expect(lifecycle).toMatchObject({
				inspections: [],
				launches: [
					{
						launch_id: "preview_browser_open_reconnect",
						state: "dispatched",
						target_id: "target_preview_browser",
					},
				],
			});
			const persisted = JSON.stringify(rows);

			expect(persisted).not.toMatch(
				/endpoint|websocket|cdp|(?:access|auth|bearer|session)_token|cookie|page_content|screenshot/i,
			);
			expect(
				rows.commands.filter((command) => command.payload_type === "preview.browser.open"),
			).toHaveLength(1);
			expect(
				rows.events.filter(
					(event) => event.event_type === "preview.browser.launch.updated",
				),
			).toHaveLength(1);

			await first.harness.dispose();
			await first.runtime.dispose();
			first = undefined;

			second = await start_stack(database_path, launcher, { transport_runtime });

			const replayed = await Effect.runPromise(
				second.harness.client.OpenPreviewInExternalBrowser(open_input),
			);
			const restored = await Effect.runPromise(
				second.harness.client.GetPreviewBrowserLifecycle({
					project_id: "project_preview_browser",
					workspace_id: "workspace_preview_browser",
				}),
			);

			expect(replayed.status).toBe("duplicate");
			expect(launcher_state.calls).toHaveLength(1);
			expect(restored).toEqual(lifecycle);
		} finally {
			if (first) {
				await first.harness.dispose();
				await first.runtime.dispose();
			}

			if (second) {
				await second.harness.dispose();
				await second.runtime.dispose();
			}
		}
	});
});
