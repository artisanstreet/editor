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
	make_backend_runtime,
	PreviewHealthProbe,
	ProtocolRouter,
	ProtocolServer,
	type ProtocolConnection,
} from "@artisan/backend";

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
		const runtime = make_backend_runtime({ database_path, migrations_path });
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
