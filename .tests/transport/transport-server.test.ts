import { MessageChannel } from "node:worker_threads";

import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
	type AckEnvelope,
	DecodeOutboundControlEnvelope,
	type CommandEnvelope,
	type EventEnvelope,
	type HelloEnvelope,
	type ThreadListQueryEnvelope,
} from "@artisan/protocol";
import {
	DecodeTransportFrame,
	type MessagePortConnection,
	type MessagePortLike,
	type TransportControlFrame,
	type TransportHelloFrame,
} from "@artisan/transport";
import { adapt_node_message_port } from "@artisan/transport/node";

import { reliable_event_window_capacity } from "../../modules/transport/src/internal/server-binding";

import { make_transport_test_harness } from "./message-channel-harness";

interface RawTransportSession {
	readonly control_port: MessagePortLike;
	readonly stream_port: MessagePortLike;
}

function make_hello(message_id = "raw_hello_1"): HelloEnvelope {
	return {
		kind: "hello",
		message_id,
		origin: "frontend",
		payload: {
			event_cursors: [],
			last_journal_sequence: 0,
			supported_protocol_versions: [1],
		},
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
	};
}

function make_command(message_id = "raw_command_1"): CommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload: { title: "Raw command", type: "thread.create" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
		thread_id: "raw_thread_1",
	};
}

function make_query(message_id = "raw_query_1"): ThreadListQueryEnvelope {
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

const open_raw_session = (
	server: Awaited<ReturnType<typeof make_transport_test_harness>>["server"],
) =>
	Effect.gen(function* () {
		const control = new MessageChannel();
		const stream = new MessageChannel();
		const client_control = yield* adapt_node_message_port(control.port1);
		const client_stream = yield* adapt_node_message_port(stream.port1);
		const server_control = yield* adapt_node_message_port(control.port2);
		const server_stream = yield* adapt_node_message_port(stream.port2);
		const server_ports: MessagePortConnection = {
			control_port: server_control,
			stream_port: server_stream,
		};

		yield* server.Serve(server_ports).pipe(Effect.exit, Effect.forkScoped);

		return {
			control_port: client_control,
			stream_port: client_stream,
		} satisfies RawTransportSession;
	});

const receive_frame = (port: MessagePortLike) =>
	port.Receive.pipe(Effect.flatMap(DecodeTransportFrame));

const bootstrap = (session: RawTransportSession) =>
	Effect.gen(function* () {
		const control_hello: TransportHelloFrame = {
			attempt_id: "raw_attempt_1",
			channel: "control",
			kind: "transport.hello",
			session_id: "raw_session_1",
			transport_version: 1,
		};
		const stream_hello: TransportHelloFrame = { ...control_hello, channel: "stream" };

		yield* Effect.all(
			[session.control_port.Send(control_hello), session.stream_port.Send(stream_hello)],
			{ concurrency: "unbounded", discard: true },
		);

		const control_ready = yield* receive_frame(session.control_port);
		const stream_ready = yield* receive_frame(session.stream_port);

		if (control_ready.kind !== "transport.ready" || stream_ready.kind !== "transport.ready") {
			return yield* Effect.die("raw transport did not become ready");
		}

		return control_ready.connection_id;
	});

const send_control = (session: RawTransportSession, connection_id: string, payload: unknown) =>
	session.control_port.Send({
		connection_id,
		kind: "transport.control",
		payload,
		transport_version: 1,
	} satisfies TransportControlFrame);

const receive_control = (session: RawTransportSession) =>
	receive_frame(session.control_port).pipe(
		Effect.flatMap((frame) =>
			frame.kind === "transport.control"
				? DecodeOutboundControlEnvelope(frame.payload)
				: Effect.die("expected a protocol control envelope"),
		),
	);

describe("MessagePort transport server validation", () => {
	it("rejects wrong transport versions during bootstrap", async () => {
		const harness = await make_transport_test_harness();

		try {
			const result = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const session = yield* open_raw_session(harness.server);
						const invalid_control = {
							attempt_id: "wrong_version_attempt",
							channel: "control",
							kind: "transport.hello",
							session_id: "wrong_version_session",
							transport_version: 2,
						};

						yield* session.control_port.Send(invalid_control);
						yield* session.stream_port.Send({ ...invalid_control, channel: "stream" });

						return yield* Effect.all(
							[session.control_port.Closed, session.stream_port.Closed],
							{ concurrency: "unbounded" },
						);
					}),
				),
			);

			expect(result).toMatchObject([{ code: "closed" }, { code: "closed" }]);
		} finally {
			await harness.dispose();
		}
	});

	it("rejects domain traffic before the transport bootstrap", async () => {
		const harness = await make_transport_test_harness();

		try {
			const close = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const session = yield* open_raw_session(harness.server);
						const stream_hello: TransportHelloFrame = {
							attempt_id: "pre_bootstrap_attempt",
							channel: "stream",
							kind: "transport.hello",
							session_id: "pre_bootstrap_session",
							transport_version: 1,
						};

						yield* session.control_port.Send({
							connection_id: "not_negotiated",
							kind: "transport.control",
							payload: make_command(),
							transport_version: 1,
						});
						yield* session.stream_port.Send(stream_hello);

						return yield* session.control_port.Closed;
					}),
				),
			);

			expect(close.code).toBe("closed");
		} finally {
			await harness.dispose();
		}
	});

	it("delegates post-bootstrap pre-hello traffic to protocol rejection", async () => {
		const harness = await make_transport_test_harness();

		try {
			const envelope = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const session = yield* open_raw_session(harness.server);
						const connection_id = yield* bootstrap(session);

						yield* send_control(session, connection_id, make_command());

						return yield* receive_control(session);
					}),
				),
			);

			expect(envelope).toMatchObject({
				correlation_id: "raw_command_1",
				kind: "protocol.error",
				payload: { code: "protocol.handshake_required", retryable: false },
			});
		} finally {
			await harness.dispose();
		}
	});

	it("fails closed on malformed, stale, and duplicate correlation frames", async () => {
		const scenarios = ["malformed", "stale", "duplicate"] as const;

		for (const scenario of scenarios) {
			const harness = await make_transport_test_harness();

			try {
				const transport_error = await Effect.runPromise(
					Effect.scoped(
						Effect.gen(function* () {
							const session = yield* open_raw_session(harness.server);
							const connection_id = yield* bootstrap(session);

							if (scenario === "malformed") {
								yield* send_control(session, connection_id, {
									kind: "not-a-protocol-frame",
								});
							} else if (scenario === "stale") {
								yield* send_control(session, "stale_connection", make_hello());
							} else {
								yield* send_control(session, connection_id, make_hello());
								yield* receive_control(session);
								yield* receive_control(session);
								yield* send_control(session, connection_id, make_query());
								yield* receive_control(session);
								yield* send_control(session, connection_id, make_query());
							}

							return yield* receive_frame(session.control_port);
						}),
					),
				);

				expect(transport_error).toMatchObject({
					channel: "control",
					code: `transport.${
						scenario === "duplicate"
							? "correlation_conflict"
							: scenario === "stale"
								? "stale_connection"
								: "malformed"
					}`,
					kind: "transport.error",
					retryable: false,
				});
			} finally {
				await harness.dispose();
			}
		}
	});

	it("closes an intentionally disposed session without a transport error", async () => {
		const harness = await make_transport_test_harness();

		try {
			const close = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const session = yield* open_raw_session(harness.server);
						const connection_id = yield* bootstrap(session);

						yield* session.control_port.Send({
							connection_id,
							kind: "transport.close",
							reason: "test_complete",
							transport_version: 1,
						});

						return yield* session.control_port.Closed;
					}),
				),
			);

			expect(close).toEqual({ code: "closed", dropped_messages: 0 });
		} finally {
			await harness.dispose();
		}
	});

	it("holds an oversized replay behind a bounded acknowledgement window", async () => {
		const harness = await make_transport_test_harness();

		try {
			for (let index = 0; index < reliable_event_window_capacity + 5; index += 1) {
				await Effect.runPromise(
					harness.client.CreateThread({ title: `Replay window ${index}` }),
				);
			}

			const output = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const session = yield* open_raw_session(harness.server);
						const connection_id = yield* bootstrap(session);
						yield* send_control(session, connection_id, make_hello("windowed_replay"));

						const welcome = yield* receive_control(session);
						const first = yield* Effect.forEach(
							Array.from({ length: reliable_event_window_capacity }),
							() => receive_control(session),
						);
						const before_ack = yield* receive_control(session).pipe(
							Effect.timeoutOption("50 millis"),
						);
						const events = first.filter(
							(envelope): envelope is EventEnvelope => envelope.kind === "event",
						);
						const last = events.at(-1);
						if (last === undefined) return yield* Effect.die("missing replay events");
						const ack: AckEnvelope = {
							kind: "ack",
							message_id: "ack_replay_window",
							origin: "frontend",
							payload: {
								event_cursors: events.map((event) => ({
									sequence: event.sequence,
									stream_id: event.stream_id,
								})),
								journal_sequence: last.journal_sequence,
							},
							protocol_version: 1,
							schema_version: 1,
							sent_at: "2026-07-10T08:00:00.000Z",
						};
						yield* send_control(session, connection_id, ack);
						const rest = yield* Effect.forEach(Array.from({ length: 6 }), () =>
							receive_control(session),
						).pipe(Effect.timeout("2 seconds"));

						return { before_ack, first, rest, welcome };
					}),
				),
			);

			expect(output.welcome.kind).toBe("welcome");
			expect(output.first).toHaveLength(reliable_event_window_capacity);
			expect(output.before_ack._tag).toBe("None");
			expect(output.rest.filter((envelope) => envelope.kind === "event")).toHaveLength(5);
			expect(output.rest.at(-1)?.kind).toBe("replay.complete");
		} finally {
			await harness.dispose();
		}
	});
});
