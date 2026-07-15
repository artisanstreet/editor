import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, Option, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	CommandEnvelope,
	HelloEnvelope,
	OutboundControlEnvelope,
	PreviewTargetsQueryEnvelope,
} from "@artisan/protocol";
import {
	BrowserInspectionConnector,
	make_backend_runtime,
	PreviewHealthProbe,
	ProtocolRouter,
	ProtocolServer,
	UnavailablePreviewHealthProbeLive,
	type ProtocolConnection,
} from "@artisan/backend";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	PreviewInspectionSessions,
	PreviewTargetRemovalClaims,
	PreviewTargetRemovalFences,
} from "../../modules/backend/src/persistence/schema";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const protocol_time = "2026-07-15T12:00:00.000Z";

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-preview-target-protocol-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function preview_command(message_id: string, payload: CommandEnvelope["payload"]): CommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: protocol_time,
		thread_id: "thread_preview_protocol",
	};
}

function preview_query(message_id: string): PreviewTargetsQueryEnvelope {
	return {
		kind: "preview.targets.query",
		message_id,
		origin: "frontend",
		payload: { project_id: "project_preview", workspace_id: "workspace_preview" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: protocol_time,
	};
}

function thread_create(): CommandEnvelope {
	return preview_command("create_preview_thread", {
		title: "Preview protocol",
		type: "thread.create",
	});
}

function hello(): HelloEnvelope {
	return {
		kind: "hello",
		message_id: "hello_preview",
		origin: "frontend",
		payload: {
			event_cursors: [],
			last_journal_sequence: 0,
			supported_protocol_versions: [1],
		},
		schema_version: 1,
		sent_at: protocol_time,
	};
}

function take_outbound(connection: ProtocolConnection, count: number) {
	return connection.Outbound.pipe(Stream.take(count), Stream.runCollect);
}

function take_until_outbound(
	connection: ProtocolConnection,
	predicate: (envelope: OutboundControlEnvelope) => boolean,
) {
	return connection.Outbound.pipe(Stream.takeUntil(predicate), Stream.runCollect);
}

const OpenConnection = Effect.gen(function* () {
	const server = yield* ProtocolServer;

	return yield* server.Open;
});

const Negotiate = (connection: ProtocolConnection) =>
	Effect.gen(function* () {
		yield* connection.Receive(hello());

		return yield* take_until_outbound(
			connection,
			(envelope) => envelope.kind === "replay.complete",
		);
	});

function route(runtime: ReturnType<typeof make_backend_runtime>, command: CommandEnvelope) {
	return runtime.runPromise(
		Effect.gen(function* () {
			const router = yield* ProtocolRouter;

			return yield* router.Route(command);
		}),
	);
}

const HealthyPreviewProbeLive = Layer.succeed(PreviewHealthProbe, {
	Probe: () =>
		Effect.succeed({
			latency_ms: 18,
			message: Option.none(),
			status: "healthy" as const,
			status_code: Option.some(200),
		}),
});

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("preview target protocol", () => {
	it("repairs a durable removal fence after post-commit claim loss", async () => {
		const database_path = await make_database_path();
		const detach_calls: Array<string> = [];

		let lose_next_claim = true;

		const connector = Layer.effect(
			BrowserInspectionConnector,
			Effect.gen(function* () {
				const database = yield* Database;

				return {
					Attach: ({ inspection_id }) =>
						Effect.succeed({
							Detach: Effect.gen(function* () {
								detach_calls.push(inspection_id);

								if (!lose_next_claim) {
									return;
								}

								lose_next_claim = false;
								yield* database.client
									.delete(PreviewTargetRemovalClaims)
									.pipe(Effect.orDie);
							}),
							Disconnected: Effect.never,
						}),
					Revoke: () => Effect.void,
				};
			}),
		).pipe(
			Layer.provide(
				make_database_layer({
					database_path,
					migrations_path,
				}),
			),
			Layer.orDie,
		);
		const runtime = make_backend_runtime({
			browser_inspection_connector: connector,
			database_path,
			migrations_path,
		});
		const remove = preview_command("remove_after_claim_loss", {
			project_id: "project_preview",
			target_id: "target_preview",
			type: "preview.target.remove",
			workspace_id: "workspace_preview",
		});

		try {
			await route(runtime, thread_create());
			await route(
				runtime,
				preview_command("register_before_claim_loss", {
					project_id: "project_preview",
					target_id: "target_preview",
					type: "preview.target.register",
					url: "http://localhost:4173/claim-loss",
					workspace_id: "workspace_preview",
				}),
			);
			await route(
				runtime,
				preview_command("attach_before_claim_loss", {
					connector_id: "connector_claim_loss",
					inspection_id: "inspection_claim_loss",
					project_id: "project_preview",
					target_id: "target_preview",
					type: "preview.inspection.attach",
					workspace_id: "workspace_preview",
				}),
			);

			const removed = await route(runtime, remove);
			const durable = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					return {
						claims: yield* database.client.select().from(PreviewTargetRemovalClaims),
						fences: yield* database.client.select().from(PreviewTargetRemovalFences),
						inspections: yield* database.client
							.select()
							.from(PreviewInspectionSessions),
					};
				}),
			);
			const replayed = await route(runtime, remove);

			expect(removed).toMatchObject([
				{ kind: "command.receipt", payload: { status: "duplicate" } },
				{
					kind: "event",
					payload: { action: "removed", type: "preview.target.updated" },
				},
			]);
			expect(replayed).toEqual([
				expect.objectContaining({
					kind: "command.receipt",
					payload: expect.objectContaining({ status: "duplicate" }),
				}),
				removed[1],
			]);
			expect(detach_calls).toEqual(["inspection_claim_loss"]);
			expect(durable.claims).toEqual([]);
			expect(durable.fences).toEqual([]);
			expect(durable.inspections).toMatchObject([
				{ reason: "target_changed", state: "disconnected" },
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("replays a removed generation while another runtime inspects its replacement", async () => {
		const database_path = await make_database_path();
		const detach_calls: Array<string> = [];
		const connector = Layer.succeed(BrowserInspectionConnector, {
			Attach: ({ inspection_id }) =>
				Effect.succeed({
					Detach: Effect.sync(() => detach_calls.push(inspection_id)),
					Disconnected: Effect.never,
				}),
			Revoke: () => Effect.void,
		});
		const first_runtime = make_backend_runtime({ database_path, migrations_path });
		const register_generation_1 = preview_command("register_generation_1", {
			project_id: "project_preview",
			target_id: "target_preview",
			type: "preview.target.register",
			url: "http://localhost:4173/generation-1",
			workspace_id: "workspace_preview",
		});
		const remove_generation_1 = preview_command("remove_generation_1", {
			project_id: "project_preview",
			target_id: "target_preview",
			type: "preview.target.remove",
			workspace_id: "workspace_preview",
		});

		let second_runtime: ReturnType<typeof make_backend_runtime> | undefined;

		try {
			await route(first_runtime, thread_create());
			await route(first_runtime, register_generation_1);

			const removed = await route(first_runtime, remove_generation_1);

			await route(
				first_runtime,
				preview_command("register_generation_2", {
					project_id: "project_preview",
					target_id: "target_preview",
					type: "preview.target.register",
					url: "http://localhost:4173/generation-2",
					workspace_id: "workspace_preview",
				}),
			);

			second_runtime = make_backend_runtime({
				browser_inspection_connector: connector,
				database_path,
				migrations_path,
			});

			const attached = await route(
				second_runtime,
				preview_command("attach_generation_2", {
					connector_id: "connector_2",
					inspection_id: "inspection_generation_2",
					project_id: "project_preview",
					target_id: "target_preview",
					type: "preview.inspection.attach",
					workspace_id: "workspace_preview",
				}),
			);
			const replayed = await route(first_runtime, remove_generation_1);
			const changed_intent = await route(
				first_runtime,
				preview_command("remove_generation_1", {
					project_id: "project_preview",
					target_id: "different_target",
					type: "preview.target.remove",
					workspace_id: "workspace_preview",
				}),
			);

			expect(attached).toMatchObject([
				{ kind: "command.receipt", payload: { status: "accepted" } },
				{
					kind: "event",
					payload: { action: "attached", type: "preview.inspection.updated" },
				},
			]);
			expect(replayed).toEqual([
				expect.objectContaining({
					kind: "command.receipt",
					payload: expect.objectContaining({ status: "duplicate" }),
				}),
				removed[1],
			]);
			expect(detach_calls).toEqual([]);
			expect(changed_intent).toMatchObject([
				{
					kind: "command.receipt",
					payload: { error: { code: "command.id_conflict" }, status: "rejected" },
				},
			]);
		} finally {
			if (second_runtime !== undefined) {
				await second_runtime.dispose();
			}

			await first_runtime.dispose();
		}
	});

	it("replays commands across restart and returns the exact scoped projection", async () => {
		const database_path = await make_database_path();
		const register = preview_command("preview_register", {
			project_id: "project_preview",
			source: { kind: "process", process_id: "process_preview" },
			target_id: "target_preview",
			type: "preview.target.register",
			url: "http://localhost:4173",
			workspace_id: "workspace_preview",
		});
		const first_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			preview_health_probe: HealthyPreviewProbeLive,
		});

		let registered: Awaited<ReturnType<typeof route>>;

		try {
			await route(first_runtime, thread_create());
			registered = await route(first_runtime, register);
			const probed = await route(
				first_runtime,
				preview_command("preview_probe", {
					project_id: "project_preview",
					target_id: "target_preview",
					type: "preview.target.probe",
					workspace_id: "workspace_preview",
				}),
			);

			expect(registered).toMatchObject([
				{
					kind: "command.receipt",
					payload: { status: "accepted" },
				},
				{
					kind: "event",
					payload: { action: "registered", type: "preview.target.updated" },
				},
			]);
			expect(probed).toMatchObject([
				{ kind: "command.receipt", payload: { status: "accepted" } },
				{
					kind: "event",
					payload: {
						action: "probed",
						target: { health: { status: "healthy", status_code: 200 } },
						type: "preview.target.updated",
					},
				},
			]);
		} finally {
			await first_runtime.dispose();
		}

		const restart_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			preview_health_probe: HealthyPreviewProbeLive,
		});

		try {
			const duplicate = await route(restart_runtime, register);

			expect(duplicate).toEqual([
				expect.objectContaining({
					kind: "command.receipt",
					payload: expect.objectContaining({ status: "duplicate" }),
				}),
				registered[1],
			]);

			await restart_runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* OpenConnection;

						yield* Negotiate(connection);
						yield* connection.Receive(preview_query("preview_query"));

						const [result] = yield* take_outbound(connection, 1);

						expect(result).toMatchObject({
							correlation_id: "preview_query",
							kind: "preview.targets.query.result",
							payload: {
								project_id: "project_preview",
								targets: [
									{
										health: { status: "healthy" },
										target_id: "target_preview",
										workspace_id: "workspace_preview",
									},
								],
								workspace_id: "workspace_preview",
							},
						});
						expect(result).not.toHaveProperty("payload.targets.0.generation_id");
					}),
				),
			);
		} finally {
			await restart_runtime.dispose();
		}
	});

	it("rejects changed command identity and unavailable health probes with source-safe errors", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			preview_health_probe: UnavailablePreviewHealthProbeLive,
		});
		const register = preview_command("preview_register_conflict", {
			project_id: "project_preview",
			target_id: "target_preview",
			type: "preview.target.register",
			url: "http://localhost:4173",
			workspace_id: "workspace_preview",
		});

		try {
			await route(runtime, thread_create());
			await route(runtime, register);
			const [missing] = await route(
				runtime,
				preview_command("preview_probe_missing", {
					project_id: "project_preview",
					target_id: "target_missing",
					type: "preview.target.probe",
					workspace_id: "workspace_preview",
				}),
			);

			const [duplicate_target] = await route(
				runtime,
				preview_command("preview_register_duplicate", {
					project_id: "project_preview",
					target_id: "target_preview",
					type: "preview.target.register",
					url: "http://localhost:4173",
					workspace_id: "workspace_preview",
				}),
			);
			const [conflict] = await route(
				runtime,
				preview_command("preview_register_conflict", {
					project_id: "project_preview",
					target_id: "target_preview",
					type: "preview.target.register",
					url: "http://localhost:4174",
					workspace_id: "workspace_preview",
				}),
			);
			const [probe] = await route(
				runtime,
				preview_command("preview_probe_unavailable", {
					project_id: "project_preview",
					target_id: "target_preview",
					type: "preview.target.probe",
					workspace_id: "workspace_preview",
				}),
			);

			expect(conflict).toMatchObject({
				kind: "command.receipt",
				payload: {
					error: { code: "command.id_conflict", retryable: false },
					status: "rejected",
				},
			});
			expect(missing).toMatchObject({
				kind: "command.receipt",
				payload: {
					error: { code: "preview.target.not_found", retryable: false },
					status: "rejected",
				},
			});
			expect(duplicate_target).toMatchObject({
				kind: "command.receipt",
				payload: {
					error: { code: "preview.target.already_exists", retryable: false },
					status: "rejected",
				},
			});
			expect(probe).toMatchObject({
				kind: "command.receipt",
				payload: {
					error: { code: "preview.target.health_probe_unavailable", retryable: true },
					status: "rejected",
				},
			});
		} finally {
			await runtime.dispose();
		}
	});
});
