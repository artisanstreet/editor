import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeWebSocketTransportFrame,
	EncodeWebSocketTransportFrame,
	make_websocket_message_port,
	type WebSocketEndpoint,
} from "@artisan/transport/websocket/protocol";

type Listener<T> = (value: T) => void;

const make_pair = () => {
	const make_endpoint = () => {
		const messages = new Set<Listener<unknown>>();
		const errors = new Set<Listener<unknown>>();
		const closes = new Set<Listener<void>>();
		let send: (data: Uint8Array) => void = (_data) => undefined;
		const endpoint: WebSocketEndpoint = {
			add_close_listener: (listener) => (closes.add(listener), () => closes.delete(listener)),
			add_error_listener: (listener) => (errors.add(listener), () => errors.delete(listener)),
			add_message_listener: (listener) => (
				messages.add(listener),
				() => messages.delete(listener)
			),
			close: () => closes.forEach((listener) => listener()),
			send: (data) => send(data),
		};
		return {
			endpoint,
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

	return [left.endpoint, right.endpoint] as const;
};

const hello = {
	attempt_id: "attempt_1",
	channel: "control" as const,
	kind: "transport.hello" as const,
	session_id: "session_1",
	transport_version: 1 as const,
};

describe("WebSocket transport framing", () => {
	it("keeps control and stream frames separated while preserving binary chunks", async () => {
		const [left_endpoint, right_endpoint] = make_pair();
		const output = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const left_control = yield* make_websocket_message_port(
						left_endpoint,
						"control",
					);
					const left_stream = yield* make_websocket_message_port(left_endpoint, "stream");
					const right_control = yield* make_websocket_message_port(
						right_endpoint,
						"control",
					);
					const right_stream = yield* make_websocket_message_port(
						right_endpoint,
						"stream",
					);
					yield* left_control.Send(hello);
					yield* left_stream.Send({
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
						control: yield* right_control.Receive,
						stream: yield* right_stream.Receive,
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
});
