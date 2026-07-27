import { Effect, Fiber } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeWebSocketTransportFrame,
	EncodeWebSocketTransportFrame,
	MakeWebSocketConnection,
	type WebSocketEndpoint,
} from "@artisan/transport/websocket/protocol";
import {
	prepare_browser_websocket,
	type BrowserWebSocket,
} from "@artisan/transport/websocket/client";

type Listener<T> = (value: T) => void;

const make_pair = () => {
	const make_endpoint = () => {
		const messages = new Set<Listener<unknown>>();
		const errors = new Set<Listener<unknown>>();
		const closes = new Set<Listener<void>>();
		let send: (data: Uint8Array) => void = (_data) => undefined;
		let close_count = 0;
		const endpoint: WebSocketEndpoint = {
			add_close_listener: (listener) => (closes.add(listener), () => closes.delete(listener)),
			add_error_listener: (listener) => (errors.add(listener), () => errors.delete(listener)),
			add_message_listener: (listener) => (
				messages.add(listener),
				() => messages.delete(listener)
			),
			close: () => {
				close_count += 1;
				closes.forEach((listener) => listener());
			},
			send: (data) => send(data),
		};
		return {
			close_count: () => close_count,
			endpoint,
			message_listener_count: () => messages.size,
			receive: (value: unknown) => messages.forEach((listener) => listener(value)),
			set_peer: (next: { readonly receive: (value: unknown) => void }) => {
				send = (data) => next.receive(data);
			},
		};
	};
	const left = make_endpoint();
	const right = make_endpoint();
	left.set_peer(right);
	right.set_peer(left);

	return [left, right] as const;
};

const hello = {
	attempt_id: "attempt_1",
	channel: "control" as const,
	kind: "transport.hello" as const,
	session_id: "session_1",
	transport_version: 1 as const,
};

describe("WebSocket transport framing", () => {
	it("requests ArrayBuffer delivery for browser binary frames", () => {
		const socket = { binaryType: "blob" } as BrowserWebSocket;

		expect(prepare_browser_websocket(socket)).toBe(socket);
		expect(socket.binaryType).toBe("arraybuffer");
	});

	it("keeps control and stream frames separated while preserving binary chunks", async () => {
		const [left, right] = make_pair();
		const output = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const left_connection = yield* MakeWebSocketConnection(left.endpoint);
					const right_connection = yield* MakeWebSocketConnection(right.endpoint);
					yield* left_connection.control_port.Send(hello);
					yield* left_connection.stream_port.Send({
						connection_id: "connection_1",
						frame: {
							channel_id: "channel_1",
							channel_sequence: 1,
							data: Uint8Array.of(1, 2, 3),
							kind: "stream.chunk",
							stream_id: "stream_1",
						},
						kind: "transport.stream",
						transport_version: 1,
					});

					return {
						control: yield* right_connection.control_port.Receive,
						stream: yield* right_connection.stream_port.Receive,
					};
				}),
			),
		);

		expect(output.control).toEqual(hello);
		expect(Array.from((output.stream as { frame: { data: Uint8Array } }).frame.data)).toEqual([
			1, 2, 3,
		]);
	});

	it("rejects malformed and wrong-channel frames before the wire protocol", async () => {
		const malformed = await Effect.runPromise(
			DecodeWebSocketTransportFrame("not json", "control").pipe(Effect.flip),
		);
		const wrong_channel = await Effect.runPromise(
			EncodeWebSocketTransportFrame("stream", hello).pipe(
				Effect.flatMap((frame) => DecodeWebSocketTransportFrame(frame, "control")),
				Effect.flip,
			),
		);

		expect(malformed).toBeInstanceOf(Error);
		expect(wrong_channel).toBeInstanceOf(Error);
	});

	it("encodes control envelopes as binary MessagePack without base64 expansion", async () => {
		const encoded = await Effect.runPromise(EncodeWebSocketTransportFrame("control", hello));
		const ascii = new TextDecoder().decode(encoded);

		expect(encoded).toBeInstanceOf(Uint8Array);
		expect(ascii).not.toContain("base64");
		expect(await Effect.runPromise(DecodeWebSocketTransportFrame(encoded, "control"))).toEqual(
			hello,
		);
	});

	it("decodes interleaved traffic once without overflowing active channel consumers", async () => {
		const [left, right] = make_pair();
		const count = 150;
		const received = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const left_connection = yield* MakeWebSocketConnection(left.endpoint, {
						incoming_capacity: 256,
					});
					const right_connection = yield* MakeWebSocketConnection(right.endpoint, {
						incoming_capacity: 256,
					});
					expect(right.message_listener_count()).toBe(1);
					const controls = yield* Effect.forEach(
						Array.from({ length: count }),
						() => right_connection.control_port.Receive,
						{ concurrency: "unbounded" },
					).pipe(Effect.forkScoped);
					const streams = yield* Effect.forEach(
						Array.from({ length: count }),
						() => right_connection.stream_port.Receive,
						{ concurrency: "unbounded" },
					).pipe(Effect.forkScoped);

					for (let index = 0; index < count; index += 1) {
						yield* left_connection.control_port.Send({
							...hello,
							attempt_id: `attempt_${index}`,
						});
						yield* left_connection.stream_port.Send({
							connection_id: "connection_1",
							frame: {
								channel_id: "channel_1",
								channel_sequence: index + 1,
								data: Uint8Array.of(index % 256),
								kind: "stream.chunk",
								stream_id: "stream_1",
							},
							kind: "transport.stream",
							transport_version: 1,
						});
						if (index % 32 === 31) {
							yield* Effect.sleep("1 millis");
						}
					}

					return {
						controls: yield* Fiber.join(controls),
						streams: yield* Fiber.join(streams),
					};
				}),
			),
		);

		expect(received.controls).toHaveLength(count);
		expect(received.streams).toHaveLength(count);
		expect((received.controls.at(-1) as { attempt_id: string }).attempt_id).toBe(
			`attempt_${count - 1}`,
		);
	});

	it("closes a shared socket exactly once when either logical port closes", async () => {
		const [left] = make_pair();
		const closed = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const connection = yield* MakeWebSocketConnection(left.endpoint);
					yield* connection.control_port.Close;
					yield* connection.stream_port.Close;
					return yield* connection.control_port.Closed;
				}),
			),
		);

		expect(closed).toEqual({ code: "closed", dropped_messages: 0 });
		expect(left.close_count()).toBe(1);
	});

	it("treats scoped decoder interruption as a normal shared close", async () => {
		const [left] = make_pair();
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					yield* MakeWebSocketConnection(left.endpoint);
					expect(left.message_listener_count()).toBe(1);
				}),
			),
		);

		expect(left.close_count()).toBe(1);
		expect(left.message_listener_count()).toBe(0);
	});
});
