import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Fiber, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	AckEnvelope,
	CommandEnvelope,
	HelloEnvelope,
	OutboundControlEnvelope,
	SubscribeEnvelope,
	ThreadListQueryEnvelope,
} from "@artisan/protocol";
import {
	DecodeProtocolConnectionOptions,
	make_backend_runtime,
	ProtocolServer,
	type ProtocolConnection,
} from "@artisan/backend";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));

const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-editor-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_hello(
	message_id = "hello_1",
	last_journal_sequence = 0,
	event_cursors: HelloEnvelope["payload"]["event_cursors"] = [],
	supported_protocol_versions: HelloEnvelope["payload"]["supported_protocol_versions"] = [1],
): HelloEnvelope {
	return {
		kind: "hello",
		message_id,
		origin: "frontend",
		payload: {
			event_cursors,
			last_journal_sequence,
			supported_protocol_versions,
		},
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
	};
}

function make_command(
	message_id = "command_1",
	thread_id = "thread_1",
	title = "Protocol integration",
): CommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload: {
			title,
			type: "thread.create",
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
		thread_id,
	};
}

function make_query(message_id = "query_1"): ThreadListQueryEnvelope {
	return {
		kind: "thread.list.query",
		message_id,
		origin: "frontend",
		payload: {},
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
	};
}

function make_subscribe(message_id = "subscribe_1"): SubscribeEnvelope {
	return {
		kind: "subscribe",
		message_id,
		origin: "frontend",
		payload: { type: "thread.list" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
		subscription_id: "subscription_1",
	};
}

function make_ack(
	message_id: string,
	journal_sequence: number,
	event_cursors: AckEnvelope["payload"]["event_cursors"],
): AckEnvelope {
	return {
		kind: "ack",
		message_id,
		origin: "frontend",
		payload: { event_cursors, journal_sequence },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
	};
}

const open_connection = Effect.gen(function* () {
	const protocol_server = yield* ProtocolServer;

	return yield* protocol_server.Open;
});

function take_outbound(connection: ProtocolConnection, count: number) {
	return connection.Outbound.pipe(Stream.take(count), Stream.runCollect);
}

function take_until_outbound(
	connection: ProtocolConnection,
	predicate: (envelope: OutboundControlEnvelope) => boolean,
) {
	return connection.Outbound.pipe(Stream.takeUntil(predicate), Stream.runCollect);
}

const negotiate = (connection: ProtocolConnection, hello = make_hello()) =>
	Effect.gen(function* () {
		yield* connection.Receive(hello);

		return yield* take_outbound(connection, 2);
	});

afterEach(async () => {
	await Promise.all(
		temporary_directories.splice(0).map((directory) =>
			rm(directory, {
				force: true,
				recursive: true,
			}),
		),
	);
});

describe("protocol server", () => {
	it("rejects traffic before hello and versionless unsupported handshakes", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const before_hello_connection = yield* open_connection;

						yield* before_hello_connection.Receive(make_command());

						const before_hello = yield* take_outbound(before_hello_connection, 1);
						const unsupported_connection = yield* open_connection;

						yield* unsupported_connection.Receive(
							make_hello("hello_unsupported", 0, [], [2]),
						);

						const unsupported = yield* take_outbound(unsupported_connection, 1);

						return { before_hello, unsupported };
					}),
				),
			);

			expect(output.before_hello).toMatchObject([
				{
					correlation_id: "command_1",
					kind: "protocol.error",
					payload: { code: "protocol.handshake_required", retryable: false },
				},
			]);
			expect(output.unsupported).toMatchObject([
				{
					correlation_id: "hello_unsupported",
					kind: "protocol.error",
					payload: { code: "protocol.unsupported_version", retryable: false },
				},
			]);
			expect(output.unsupported[0]).not.toHaveProperty("protocol_version");
		} finally {
			await runtime.dispose();
		}
	});

	it("negotiates, returns a query snapshot, and validates acknowledgements", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;
						const handshake = yield* negotiate(connection);

						yield* connection.Receive(make_command());

						const command = yield* take_outbound(connection, 2);

						yield* connection.Receive(make_query());

						const query = yield* take_outbound(connection, 1);

						yield* connection.Receive(
							make_ack("ack_valid", 1, [
								{ sequence: 1, stream_id: "thread:thread_1" },
							]),
						);
						yield* connection.Receive(
							make_ack("ack_duplicate_cursor", 1, [
								{ sequence: 1, stream_id: "thread:thread_1" },
								{ sequence: 1, stream_id: "thread:thread_1" },
							]),
						);
						yield* connection.Receive(
							make_ack("ack_projection_cursor", 1, [
								{
									sequence: 1,
									stream_id: "projection:thread.list:subscription_1",
								},
							]),
						);
						yield* connection.Receive(
							make_ack("ack_out_of_range", 2, [
								{ sequence: 2, stream_id: "thread:thread_1" },
							]),
						);

						const acknowledgement = yield* take_outbound(connection, 3);

						return { acknowledgement, command, handshake, query };
					}),
				),
			);

			expect(output.handshake.map((envelope) => envelope.kind)).toEqual([
				"welcome",
				"replay.complete",
			]);
			expect(output.command).toMatchObject([
				{
					correlation_id: "command_1",
					kind: "command.receipt",
					payload: { journal_sequence: 1, status: "accepted" },
				},
				{
					correlation_id: "command_1",
					journal_sequence: 1,
					kind: "event",
				},
			]);
			expect(output.query).toMatchObject([
				{
					correlation_id: "query_1",
					kind: "thread.list.query.result",
					payload: {
						journal_sequence: 1,
						threads: [{ thread_id: "thread_1", title: "Protocol integration" }],
					},
				},
			]);
			expect(
				output.acknowledgement.map((envelope) =>
					"correlation_id" in envelope ? envelope.correlation_id : undefined,
				),
			).toEqual(["ack_duplicate_cursor", "ack_projection_cursor", "ack_out_of_range"]);
			expect(output.acknowledgement).toMatchObject(
				Array.from({ length: 3 }, () => ({
					kind: "protocol.error",
					payload: { code: "protocol.invalid_ack", retryable: false },
				})),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("sends subscription snapshots and does not replay duplicate commands live", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* negotiate(connection);
						yield* connection.Receive(make_subscribe());

						const subscription = yield* take_outbound(connection, 2);

						yield* connection.Receive(make_command());

						const accepted = yield* take_outbound(connection, 3);
						const duplicate_delivery = yield* take_until_outbound(
							connection,
							(envelope) => envelope.kind === "thread.list.query.result",
						).pipe(Effect.forkChild);

						yield* connection.Receive(make_command());
						yield* connection.Receive(make_query("query_after_duplicate"));

						const after_duplicate = yield* Fiber.join(duplicate_delivery);

						return { accepted, after_duplicate, subscription };
					}),
				),
			);

			expect(output.subscription).toMatchObject([
				{
					correlation_id: "subscribe_1",
					kind: "subscription.started",
					subscription_id: "subscription_1",
				},
				{
					kind: "thread.list.snapshot",
					payload: { threads: [] },
					sequence: 0,
					subscription_id: "subscription_1",
				},
			]);
			expect(output.accepted).toMatchObject([
				{
					kind: "command.receipt",
					payload: { status: "accepted" },
				},
				{ kind: "event", journal_sequence: 1 },
				{
					kind: "thread.list.upsert",
					payload: { thread_id: "thread_1", title: "Protocol integration" },
					sequence: 1,
				},
			]);
			expect(output.after_duplicate.map((envelope) => envelope.kind)).toEqual([
				"command.receipt",
				"thread.list.query.result",
			]);
			expect(output.after_duplicate).toMatchObject([
				{
					correlation_id: "command_1",
					kind: "command.receipt",
					payload: { status: "duplicate" },
				},
				{
					correlation_id: "query_after_duplicate",
					kind: "thread.list.query.result",
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("reconnects from a cursor and replays only missing events", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const replay = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const initial_connection = yield* open_connection;

						yield* negotiate(initial_connection);
						yield* initial_connection.Receive(make_command());
						yield* take_outbound(initial_connection, 2);
						yield* initial_connection.Close;

						const producer_connection = yield* open_connection;

						yield* negotiate(producer_connection, make_hello("hello_producer"));
						yield* producer_connection.Receive(
							make_command("command_2", "thread_2", "Missing event"),
						);
						yield* take_outbound(producer_connection, 2);

						const reconnecting_connection = yield* open_connection;

						yield* reconnecting_connection.Receive(
							make_hello("hello_reconnect", 1, [
								{ sequence: 1, stream_id: "thread:thread_1" },
							]),
						);

						return yield* take_outbound(reconnecting_connection, 3);
					}),
				),
			);

			expect(replay.map((envelope) => envelope.kind)).toEqual([
				"welcome",
				"event",
				"replay.complete",
			]);
			expect(replay[1]).toMatchObject({
				journal_sequence: 2,
				payload: { title: "Missing event", type: "thread.created" },
				thread_id: "thread_2",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("broadcasts committed events to two live connections", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const observer_event = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const source_connection = yield* open_connection;
						const observer_connection = yield* open_connection;

						yield* negotiate(source_connection, make_hello("hello_source"));
						yield* negotiate(observer_connection, make_hello("hello_observer"));
						yield* source_connection.Receive(make_command());
						yield* take_outbound(source_connection, 2);

						return yield* take_outbound(observer_connection, 1);
					}),
				),
			);

			expect(observer_event).toMatchObject([
				{
					correlation_id: "command_1",
					journal_sequence: 1,
					kind: "event",
					thread_id: "thread_1",
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects malformed negotiated input and closes connections with their scope", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const malformed = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* negotiate(connection);
						yield* connection.Receive({
							kind: "command",
							message_id: "malformed_command",
							origin: "frontend",
							protocol_version: 1,
							schema_version: 1,
							sent_at: "not-an-iso-date",
						});

						return yield* take_outbound(connection, 1);
					}),
				),
			);
			const closed_connection = await runtime.runPromise(Effect.scoped(open_connection));

			await runtime.runPromise(closed_connection.Closed);

			expect(malformed).toMatchObject([
				{
					kind: "protocol.error",
					payload: { code: "protocol.invalid_message", retryable: false },
					protocol_version: 1,
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("validates connection settings before constructing the server", async () => {
		const invalid_capacity = await Effect.runPromise(
			DecodeProtocolConnectionOptions({
				heartbeat_interval_ms: 10,
				heartbeat_timeout_ms: 20,
				outbound_capacity: 0,
			}).pipe(Effect.flip),
		);
		const invalid_timeout = await Effect.runPromise(
			DecodeProtocolConnectionOptions({
				heartbeat_interval_ms: 20,
				heartbeat_timeout_ms: 10,
				outbound_capacity: 1,
			}).pipe(Effect.flip),
		);

		expect(invalid_capacity._tag).toBe("ProtocolConfigurationError");
		expect(invalid_timeout._tag).toBe("ProtocolConfigurationError");
	});
});
