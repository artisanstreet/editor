import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Exit, Fiber, Option, Stream } from "effect";
import { TestClock } from "effect/testing";
import { afterEach, describe, expect, it } from "vitest";

import type {
	CommandEnvelope,
	HelloEnvelope,
	OutboundControlEnvelope,
	ThreadListQueryEnvelope,
} from "@artisan/protocol";
import {
	make_backend_runtime,
	ProtocolServer,
	type ProtocolConnection,
	type ProtocolConnectionOptions,
} from "@artisan/backend";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));

const temporary_directories: Array<string> = [];

const heartbeat_options: ProtocolConnectionOptions = {
	heartbeat_interval_ms: 10,
	heartbeat_timeout_ms: 20,
};

type HeartbeatPingEnvelope = Extract<OutboundControlEnvelope, { readonly kind: "heartbeat.ping" }>;

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-editor-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

async function with_backend<A>(
	protocol: Partial<ProtocolConnectionOptions>,
	run: (runtime: ReturnType<typeof make_backend_runtime>) => Promise<A>,
) {
	const database_path = await make_database_path();
	const runtime = make_backend_runtime({
		database_path,
		migrations_path,
		protocol,
	});

	try {
		return await run(runtime);
	} finally {
		await runtime.dispose();
	}
}

function make_hello(
	message_id = "hello_1",
	supported_protocol_versions: HelloEnvelope["payload"]["supported_protocol_versions"] = [1],
): HelloEnvelope {
	return {
		kind: "hello",
		message_id,
		origin: "frontend",
		payload: {
			event_cursors: [],
			last_journal_sequence: 0,
			supported_protocol_versions,
		},
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
	};
}

function make_command(message_id = "command_1", thread_id = "thread_1"): CommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload: {
			title: "Closed connections cannot write",
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

function make_pong(ping: HeartbeatPingEnvelope, message_id = "pong_1", nonce = ping.payload.nonce) {
	return {
		correlation_id: ping.message_id,
		kind: "heartbeat.pong",
		message_id,
		origin: "frontend",
		payload: { nonce },
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

function take_ping(connection: ProtocolConnection) {
	return take_outbound(connection, 1).pipe(
		Effect.flatMap(([envelope]) =>
			envelope?.kind === "heartbeat.ping"
				? Effect.succeed(envelope)
				: Effect.die(`Expected heartbeat.ping, received ${envelope?.kind ?? "nothing"}`),
		),
	);
}

const negotiate = (connection: ProtocolConnection, hello = make_hello()) =>
	Effect.gen(function* () {
		yield* connection.Receive(hello);

		return yield* connection.Outbound.pipe(
			Stream.takeUntil((envelope) => envelope.kind === "replay.complete"),
			Stream.runCollect,
		);
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

describe("protocol connection lifecycle", () => {
	it("explicit Close completes Closed and terminates Outbound", async () => {
		const outbound = await with_backend({}, (runtime) =>
			runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;
						const outbound_fiber = yield* connection.Outbound.pipe(
							Stream.runCollect,
							Effect.forkChild,
						);
						const closed_fiber = yield* connection.Closed.pipe(Effect.forkChild);

						yield* connection.Close;
						yield* Fiber.join(closed_fiber);

						return yield* Fiber.await(outbound_fiber);
					}),
				),
			),
		);

		expect(Exit.isSuccess(outbound)).toBe(true);
		expect(Exit.isSuccess(outbound) ? outbound.value : []).toHaveLength(0);
	});

	it("Close racing concurrent Receive calls cannot resurrect a connection", async () => {
		const outbound = await with_backend({}, (runtime) =>
			runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* negotiate(connection);

						const outbound_fiber = yield* connection.Outbound.pipe(
							Stream.runCollect,
							Effect.forkChild,
						);
						const receives = Effect.forEach(
							Array.from({ length: 64 }, (_, index) => index),
							(index) => connection.Receive(make_query(`query_${index}`)),
							{ concurrency: "unbounded", discard: true },
						);

						yield* Effect.all([connection.Close, receives], {
							concurrency: "unbounded",
							discard: true,
						});
						yield* connection.Receive(make_query("query_after_close"));
						yield* connection.Closed;

						return yield* Fiber.await(outbound_fiber);
					}),
				),
			),
		);

		expect(Exit.isSuccess(outbound)).toBe(true);
	});

	it("does not accept commands received after Close into durable state", async () => {
		const query = await with_backend({}, (runtime) =>
			runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const closed_connection = yield* open_connection;

						yield* negotiate(closed_connection);
						yield* closed_connection.Close;
						yield* closed_connection.Receive(make_command());

						const reconnecting_connection = yield* open_connection;

						yield* negotiate(reconnecting_connection, make_hello("hello_reconnect"));
						yield* reconnecting_connection.Receive(make_query("query_reconnect"));

						return yield* take_outbound(reconnecting_connection, 1);
					}),
				),
			),
		);

		/** The guidance anchor occupies journal sequence 1; the refused command adds nothing. */
		expect(query).toMatchObject([
			{
				correlation_id: "query_reconnect",
				kind: "thread.list.query.result",
				payload: { journal_sequence: 1, threads: [] },
			},
		]);
	});

	it("pings after idle, accepts a valid pong, errors on an invalid pong, and closes after a missing pong", async () => {
		const output = await with_backend(heartbeat_options, (runtime) =>
			runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* negotiate(connection);
						yield* TestClock.adjust(10);

						const first_ping = yield* take_ping(connection);

						yield* connection.Receive(make_pong(first_ping));
						yield* TestClock.adjust(10);

						const second_ping = yield* take_ping(connection);

						yield* connection.Receive(
							make_pong(second_ping, "pong_invalid", "wrong_nonce"),
						);

						const invalid_pong = yield* take_outbound(connection, 1);

						yield* TestClock.adjust(10);

						const closed_before_second_deadline = yield* connection.Closed.pipe(
							Effect.timeoutOption(1),
							Effect.forkChild,
						);

						yield* TestClock.adjust(1);

						const early_close = yield* Fiber.join(closed_before_second_deadline);

						yield* TestClock.adjust(9);
						yield* connection.Closed;

						return { early_close, first_ping, invalid_pong, second_ping };
					}),
				).pipe(Effect.provide(TestClock.layer())),
			),
		);

		expect(output.first_ping.kind).toBe("heartbeat.ping");
		expect(output.second_ping.kind).toBe("heartbeat.ping");
		expect(output.early_close).toEqual(Option.none());
		expect(output.invalid_pong).toMatchObject([
			{
				correlation_id: "pong_invalid",
				kind: "protocol.error",
				payload: { code: "protocol.invalid_heartbeat", retryable: false },
			},
		]);
	});

	it("closes a rejected pre-negotiation connection after its timeout", async () => {
		const rejected = await with_backend(heartbeat_options, (runtime) =>
			runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* open_connection;

						yield* connection.Receive(make_hello("hello_rejected", [2]));

						const error = yield* take_outbound(connection, 1);

						yield* TestClock.adjust(20);
						yield* connection.Closed;

						return error;
					}),
				).pipe(Effect.provide(TestClock.layer())),
			),
		);

		expect(rejected).toMatchObject([
			{
				correlation_id: "hello_rejected",
				kind: "protocol.error",
				payload: { code: "protocol.unsupported_version", retryable: false },
			},
		]);
	});
});
