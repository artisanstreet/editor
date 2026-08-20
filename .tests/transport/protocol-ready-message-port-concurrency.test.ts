import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { MessageChannel } from "node:worker_threads";

import { Deferred, Effect, Layer, Ref } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	AckEnvelope,
	HeartbeatPingEnvelope,
	HeartbeatPongEnvelope,
	HelloEnvelope,
	SubscribeEnvelope,
	ThreadListQueryEnvelope,
} from "@artisan/protocol";
import { DecodeOutboundControlEnvelope } from "@artisan/protocol";
import { make_backend_runtime, ProtocolServer } from "@artisan/backend";
import {
	DecodeTransportFrame,
	type MessagePortConnection,
	type MessagePortLike,
	type TransportControlFrame,
	type TransportHelloFrame,
} from "@artisan/transport";
import { adapt_node_message_port } from "@artisan/transport/node";

import { make_transport_test_harness_with_protocol_server } from "./message-channel-harness";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import { RuntimeMetadata, type RuntimeIdPrefix } from "../../modules/backend/src/runtime/metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

interface RawTransportSession {
	readonly control_port: MessagePortLike;
	readonly stream_port: MessagePortLike;
}

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-message-port-concurrency-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

const MakeHello = (): HelloEnvelope => ({
	kind: "hello",
	message_id: "message_port_hello",
	origin: "frontend",
	payload: {
		event_cursors: [],
		last_journal_sequence: 0,
		resume_mode: "fresh",
		supported_protocol_versions: [1],
	},
	schema_version: 1,
	sent_at: "2026-08-15T08:00:00.000Z",
});

const MakeThreadListProjection = (): SubscribeEnvelope => ({
	kind: "subscribe",
	message_id: "message_port_thread_list_projection",
	origin: "frontend",
	payload: { type: "thread.list" },
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-08-15T08:00:00.000Z",
	subscription_id: "message_port_thread_list_subscription",
});

const MakeThreadList = (): ThreadListQueryEnvelope => ({
	kind: "thread.list.query",
	message_id: "message_port_thread_list",
	origin: "frontend",
	payload: {},
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-08-15T08:00:00.000Z",
});

const MakeAck = (
	message_id: string,
	journal_sequence: number,
	event_cursors: AckEnvelope["payload"]["event_cursors"],
): AckEnvelope => ({
	kind: "ack",
	message_id,
	origin: "frontend",
	payload: { event_cursors, journal_sequence },
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-08-15T08:00:00.000Z",
});

const MakePong = (ping: HeartbeatPingEnvelope): HeartbeatPongEnvelope => ({
	correlation_id: ping.message_id,
	kind: "heartbeat.pong",
	message_id: "message_port_pong",
	origin: "frontend",
	payload: { nonce: ping.payload.nonce },
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-08-15T08:00:00.000Z",
});

const OpenRawSession = (
	server: Awaited<ReturnType<typeof make_transport_test_harness_with_protocol_server>>["server"],
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

const ReceiveFrame = (port: MessagePortLike) =>
	port.Receive.pipe(Effect.flatMap(DecodeTransportFrame));

const ReceiveControl = (session: RawTransportSession) =>
	ReceiveFrame(session.control_port).pipe(
		Effect.flatMap((frame) =>
			frame.kind === "transport.control"
				? DecodeOutboundControlEnvelope(frame.payload)
				: Effect.die("expected a protocol control envelope"),
		),
	);

const Bootstrap = (session: RawTransportSession) =>
	Effect.gen(function* () {
		const control_hello: TransportHelloFrame = {
			attempt_id: "message_port_attempt",
			channel: "control",
			kind: "transport.hello",
			session_id: "message_port_session",
			transport_version: 1,
		};
		const stream_hello: TransportHelloFrame = { ...control_hello, channel: "stream" };

		yield* Effect.all(
			[session.control_port.Send(control_hello), session.stream_port.Send(stream_hello)],
			{ concurrency: "unbounded", discard: true },
		);

		const control_ready = yield* ReceiveFrame(session.control_port);
		const stream_ready = yield* ReceiveFrame(session.stream_port);

		if (control_ready.kind !== "transport.ready" || stream_ready.kind !== "transport.ready") {
			return yield* Effect.die("raw transport did not become ready");
		}

		return control_ready.connection_id;
	});

const SendControl = (session: RawTransportSession, connection_id: string, payload: unknown) =>
	session.control_port.Send({
		connection_id,
		kind: "transport.control",
		payload,
		transport_version: 1,
	} satisfies TransportControlFrame);

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("ProtocolServer MessagePort admission", () => {
	it("admits pong and ACK control behind a blocked live projection and interrupts it on close", async () => {
		const database_path = await make_database_path();
		const projection_started = await Effect.runPromise(Deferred.make<void>());
		const projection_release = await Effect.runPromise(Deferred.make<void>());
		const projection_interrupted = await Effect.runPromise(Deferred.make<void>());
		const projection_completed = await Effect.runPromise(Deferred.make<void>());
		const block_projection = await Effect.runPromise(Ref.make(false));
		const identifier_count = await Effect.runPromise(Ref.make(0));
		const runtime_metadata = Layer.succeed(
			RuntimeMetadata,
			RuntimeMetadata.of({
				instance_id: "message_port_backend",
				MakeId: (prefix: RuntimeIdPrefix) =>
					Ref.getAndUpdate(identifier_count, (count) => count + 1).pipe(
						Effect.flatMap((count) => {
							if (prefix !== "message") return Effect.succeed(`${prefix}_${count}`);
							return Ref.getAndSet(block_projection, false).pipe(
								Effect.flatMap((should_block) =>
									!should_block
										? Effect.succeed(`message_${count}`)
										: Deferred.succeed(projection_started, undefined).pipe(
												Effect.andThen(Deferred.await(projection_release)),
												Effect.tap(() =>
													Deferred.succeed(
														projection_completed,
														undefined,
													),
												),
												Effect.as(`message_${count}`),
												Effect.onInterrupt(() =>
													Deferred.succeed(
														projection_interrupted,
														undefined,
													),
												),
											),
								),
							);
						}),
					),
				Now: Effect.succeed("2026-08-15T08:00:00.000Z"),
			}),
		);
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			protocol: { heartbeat_interval_ms: 50, heartbeat_timeout_ms: 2_000 },
			runtime_metadata,
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);

		try {
			const result = await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const session = yield* OpenRawSession(harness.server);
						const connection_id = yield* Bootstrap(session);

						yield* SendControl(session, connection_id, MakeHello());
						yield* ReceiveControl(session);
						yield* ReceiveControl(session);

						yield* SendControl(session, connection_id, MakeThreadListProjection());
						yield* Effect.gen(function* () {
							let snapshots = 0;
							while (snapshots < 2) {
								const envelope = yield* ReceiveControl(session);
								if (
									envelope.kind === "subscription.started" ||
									envelope.kind === "thread.list.snapshot"
								)
									snapshots += 1;
							}
						});
						yield* Ref.set(block_projection, true);
						const event = yield* Effect.promise(() =>
							runtime.runPromise(
								Effect.gen(function* () {
									const journal = yield* JournalStore;
									return yield* journal.AcceptThreadCreate({
										kind: "command",
										message_id: "message_port_event_command",
										origin: "frontend",
										payload: { title: "Event ACK", type: "thread.create" },
										protocol_version: 1,
										schema_version: 1,
										sent_at: "2026-08-15T08:00:00.000Z",
										thread_id: "message_port_event_thread",
									});
								}),
							),
						);
						yield* Deferred.await(projection_started);
						const delivered_event = yield* Effect.gen(function* () {
							while (true) {
								const envelope = yield* ReceiveControl(session);
								if (envelope.kind === "event") return envelope;
							}
						});
						yield* SendControl(
							session,
							connection_id,
							MakeAck("message_port_valid_ack", delivered_event.journal_sequence, [
								{
									sequence: delivered_event.sequence,
									stream_id: delivered_event.stream_id,
								},
							]),
						);
						yield* SendControl(
							session,
							connection_id,
							MakeAck(
								"message_port_duplicate_ack",
								delivered_event.journal_sequence,
								[
									{
										sequence: delivered_event.sequence,
										stream_id: delivered_event.stream_id,
									},
								],
							),
						);
						const ping = yield* Effect.gen(function* () {
							while (true) {
								const envelope = yield* ReceiveControl(session);
								if (envelope.kind === "heartbeat.ping") return envelope;
							}
						});
						yield* SendControl(session, connection_id, MakePong(ping));
						yield* SendControl(session, connection_id, MakeThreadList());
						const query_response = yield* Effect.gen(function* () {
							while (true) {
								const envelope = yield* ReceiveControl(session);
								if (
									envelope.kind === "protocol.error" &&
									(envelope.payload.code === "protocol.invalid_ack" ||
										envelope.payload.code === "protocol.invalid_heartbeat") &&
									(envelope.correlation_id === "message_port_valid_ack" ||
										envelope.correlation_id === "message_port_duplicate_ack" ||
										envelope.correlation_id === "message_port_pong")
								)
									return yield* Effect.die(
										"a valid ACK or matching heartbeat response was rejected",
									);
								if (envelope.kind === "thread.list.query.result") return envelope;
							}
						});
						yield* SendControl(
							session,
							connection_id,
							MakeAck(
								"message_port_invalid_ack",
								delivered_event.journal_sequence,
								[],
							),
						);
						const invalid_ack = yield* Effect.gen(function* () {
							while (true) {
								const envelope = yield* ReceiveControl(session);
								if (
									envelope.kind === "protocol.error" &&
									(envelope.payload.code === "protocol.invalid_ack" ||
										envelope.payload.code === "protocol.invalid_heartbeat") &&
									(envelope.correlation_id === "message_port_valid_ack" ||
										envelope.correlation_id === "message_port_duplicate_ack" ||
										envelope.correlation_id === "message_port_pong")
								)
									return yield* Effect.die(
										"a valid ACK or matching heartbeat response was rejected",
									);
								if (
									envelope.kind === "protocol.error" &&
									envelope.correlation_id === "message_port_invalid_ack"
								)
									return envelope;
							}
						});

						yield* session.control_port.Send({
							connection_id,
							kind: "transport.close",
							reason: "close_blocked_picker",
							transport_version: 1,
						});
						yield* Deferred.await(projection_interrupted);
						const closed = yield* session.control_port.Closed;
						yield* Deferred.succeed(projection_release, undefined);
						const completed_after_close = yield* Deferred.isDone(projection_completed);

						return {
							closed,
							completed_after_close,
							event,
							invalid_ack,
							query_response,
						};
					}),
				),
			);

			expect(result.query_response).toMatchObject({
				correlation_id: "message_port_thread_list",
				kind: "thread.list.query.result",
				payload: { threads: [{ thread_id: "message_port_event_thread" }] },
			});
			expect(result.invalid_ack).toMatchObject({
				correlation_id: "message_port_invalid_ack",
				kind: "protocol.error",
				payload: { code: "protocol.invalid_ack" },
			});
			expect(result.closed).toEqual({ code: "closed", dropped_messages: 0 });
			expect(result.completed_after_close).toBe(false);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	});
});
