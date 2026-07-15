import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, Fiber, FileSystem, Layer, Option, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	make_backend_runtime,
	PreviewHealthProbe,
	ProtocolRouter,
	ProtocolServer,
} from "@artisan/backend";
import { TransportRuntime } from "@artisan/transport";

import { Database } from "../../modules/backend/src/persistence/database";
import { JournalCommands, JournalEvents } from "../../modules/backend/src/persistence/schema";
import {
	make_transport_test_harness_with_protocol_server,
	wait_for,
} from "./message-channel-harness";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const now = "2026-07-15T12:00:00.000Z";

const MakeDatabasePath = Effect.flatMap(FileSystem.FileSystem, (file_system) =>
	file_system.makeTempDirectory({ prefix: "artisan-preview-transport-" }).pipe(
		Effect.tap((path) => Effect.sync(() => temporary_directories.push(path))),
		Effect.map((path) => `${path}/artisan.db`),
	),
);

function make_probe_layer(state: { calls: number }) {
	return Layer.succeed(PreviewHealthProbe, {
		Probe: () =>
			Effect.sync(() => {
				state.calls += 1;

				return {
					latency_ms: 12,
					message: Option.none<string>(),
					status: "healthy" as const,
					status_code: Option.some(200),
				};
			}),
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
	probe: Layer.Layer<PreviewHealthProbe>,
	options: {
		readonly drop_first_command_receipt?: boolean;
		readonly initialize_thread?: boolean;
		readonly transport_runtime?: Layer.Layer<TransportRuntime>;
	} = {},
) {
	const runtime = make_backend_runtime({
		database_path,
		migrations_path,
		preview_health_probe: probe,
	});
	const protocol_server = await runtime.runPromise(ProtocolServer);

	if (options.initialize_thread) {
		const router = await runtime.runPromise(ProtocolRouter);

		await runtime.runPromise(
			router.Route({
				kind: "command",
				message_id: "preview_thread_create",
				origin: "frontend",
				payload: { title: "Preview transport", type: "thread.create" },
				protocol_version: 1,
				schema_version: 1,
				sent_at: now,
				thread_id: "thread_preview",
			}),
		);
	}

	const harness = await make_transport_test_harness_with_protocol_server(protocol_server, {
		client: { reconnect_delay_ms: 5 },
		...(options.drop_first_command_receipt === undefined
			? {}
			: { drop_first_command_receipt: options.drop_first_command_receipt }),
		...(options.transport_runtime === undefined
			? {}
			: { transport_runtime: options.transport_runtime }),
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

describe("ArtisanClient preview targets with the backend ProtocolServer", () => {
	it("replays exact commands over reconnect and restart without repeating probe work", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const probe_state = { calls: 0 };
		const probe = make_probe_layer(probe_state);
		const transport_runtime = make_transport_runtime_layer();
		const register_input = {
			command_id: "preview_register_reconnect",
			project_id: "project_preview",
			source: { kind: "process" as const, process_id: "process_preview" },
			target_id: "target_preview",
			thread_id: "thread_preview",
			url: "http://127.0.0.1:4173/app",
			workspace_id: "workspace_preview",
		};
		const probe_input = {
			command_id: "preview_probe_restart",
			project_id: "project_preview",
			target_id: "target_preview",
			thread_id: "thread_preview",
			workspace_id: "workspace_preview",
		};
		let first: Awaited<ReturnType<typeof start_stack>> | undefined = await start_stack(
			database_path,
			probe,
			{
				drop_first_command_receipt: true,
				initialize_thread: true,
				transport_runtime,
			},
		);
		let second: Awaited<ReturnType<typeof start_stack>> | undefined;

		try {
			if (!first) {
				throw new Error("The first preview transport stack was not started");
			}

			const current_first = first;
			const transport_output = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const events_fiber = yield* current_first.harness.client.Events.pipe(
							Stream.filter(
								(event) => event.payload.type === "preview.target.updated",
							),
							Stream.take(2),
							Stream.runCollect,
							Effect.forkScoped,
						);
						const registered =
							yield* current_first.harness.client.RegisterPreviewTarget(
								register_input,
							);

						yield* Effect.promise(() =>
							wait_for(
								() => current_first.harness.connector_snapshot().connections === 2,
							),
						);

						const probed =
							yield* current_first.harness.client.ProbePreviewTarget(probe_input);
						const events = yield* Fiber.join(events_fiber);

						return {
							cursors: yield* current_first.harness.client.Cursors,
							events: [...events],
							probed,
							registered,
						};
					}),
				),
			);
			const before_restart = await Effect.runPromise(
				current_first.harness.client.GetPreviewTargets({
					project_id: "project_preview",
					workspace_id: "workspace_preview",
				}),
			);
			const first_rows = await current_first.runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					return {
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
					};
				}),
			);

			expect(transport_output.registered.status).toBe("duplicate");
			expect(transport_output.probed.status).toBe("accepted");
			expect(current_first.harness.connector_snapshot()).toMatchObject({
				connections: 2,
				dropped_command_receipts: 1,
			});
			expect(transport_output.events).toMatchObject([
				{
					correlation_id: "preview_register_reconnect",
					payload: { action: "registered", type: "preview.target.updated" },
					sequence: 2,
					stream_id: "thread:thread_preview",
				},
				{
					correlation_id: "preview_probe_restart",
					payload: { action: "probed", type: "preview.target.updated" },
					sequence: 3,
					stream_id: "thread:thread_preview",
				},
			]);
			expect(transport_output.events[1]!.journal_sequence).toBeGreaterThan(
				transport_output.events[0]!.journal_sequence,
			);
			expect(transport_output.cursors.event_cursors).toContainEqual({
				sequence: 3,
				stream_id: "thread:thread_preview",
			});
			expect(transport_output.cursors.last_journal_sequence).toBeGreaterThanOrEqual(
				transport_output.events[1]!.journal_sequence,
			);
			expect(
				first_rows.commands
					.filter((command) => command.payload_type.startsWith("preview.target."))
					.map((command) => command.message_id),
			).toEqual(["preview_register_reconnect", "preview_probe_restart"]);
			expect(
				first_rows.events
					.filter((event) => event.event_type === "preview.target.updated")
					.map((event) => event.correlation_id),
			).toEqual(["preview_register_reconnect", "preview_probe_restart"]);
			expect(before_restart.targets).toMatchObject([
				{
					health: { status: "healthy", status_code: 200 },
					target_id: "target_preview",
				},
			]);
			expect(probe_state.calls).toBe(1);

			await current_first.harness.dispose();
			await current_first.runtime.dispose();
			first = undefined;

			second = await start_stack(database_path, probe, { transport_runtime });

			const replayed_probe = await Effect.runPromise(
				second.harness.client.ProbePreviewTarget(probe_input),
			);
			const restored = await Effect.runPromise(
				second.harness.client.GetPreviewTargets({
					project_id: "project_preview",
					workspace_id: "workspace_preview",
				}),
			);

			expect(replayed_probe.status).toBe("duplicate");
			expect(probe_state.calls).toBe(1);
			expect(restored).toEqual(before_restart);
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
