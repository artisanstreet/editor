import { Deferred, Effect, Fiber, Option, Schema, Stream } from "effect";
import { describe, expect, it } from "vitest";

import {
	ArtisanClientError,
	DecodeMessagePortStreamFrame,
	DecodeTransportFrame,
	MessagePortStreamEndFrame,
	type MessagePortConnection,
	type MessagePortLike,
} from "@artisan/transport";

import { make_client_stream_channel } from "../../../modules/transport/src/internal/client-stream-channel";

function make_fake_port(
	sent: Array<unknown>,
	on_send: Effect.Effect<void> = Effect.void,
): MessagePortLike {
	return {
		Close: Effect.void,
		Closed: Effect.never,
		Receive: Effect.never,
		Send: (message) =>
			Effect.sync(() => {
				sent.push(message);
			}).pipe(Effect.andThen(on_send)),
	};
}

describe("client stream channel", () => {
	it("decodes a logical not-found close frame", async () => {
		const stream_frame = {
			channel_id: "binary_stream_1",
			channel_sequence: 0,
			kind: "stream.end",
			reason: "not_found",
			stream_id: "asset_1",
		} as const;
		await Effect.runPromise(
			Schema.decodeUnknownEffect(MessagePortStreamEndFrame)(stream_frame),
		);
		await Effect.runPromise(DecodeMessagePortStreamFrame(stream_frame));
		const frame = await Effect.runPromise(
			DecodeTransportFrame({
				connection_id: "connection_1",
				frame: stream_frame,
				kind: "transport.stream",
				transport_version: 1,
			}),
		);

		expect(frame).toMatchObject({ frame: { kind: "stream.end", reason: "not_found" } });
	});

	it("fails a full client queue and cancels its exact logical stream", async () => {
		const sent: Array<unknown> = [];
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const bound = yield* Deferred.make<void>();
					const stream_port = make_fake_port(
						sent,
						Deferred.succeed(bound, undefined).pipe(Effect.asVoid),
					);
					const ports: MessagePortConnection = {
						control_port: make_fake_port([]),
						stream_port,
					};
					const active = {
						connection_id: "connection_1",
						ports,
						protocol_connection_id: "protocol_connection_1",
						stream_ticket: "stream_ticket_1",
					};
					const channel = yield* make_client_stream_channel(
						1,
						() => Effect.succeed("binary_stream_1"),
						Effect.succeed(active),
						Effect.succeed(Option.some(active)),
					);
					const opening = yield* channel.Open("asset_1").pipe(Effect.forkScoped);
					yield* Deferred.await(bound);

					yield* channel.Handle(active, {
						channel_id: "binary_stream_1",
						channel_sequence: 0,
						kind: "stream.ready",
						stream_id: "asset_1",
					});
					const stream = yield* Fiber.join(opening);

					yield* channel.Handle(active, {
						channel_id: "binary_stream_1",
						channel_sequence: 1,
						data: Uint8Array.of(1),
						kind: "stream.chunk",
						stream_id: "asset_1",
					});
					yield* channel.Handle(active, {
						channel_id: "binary_stream_1",
						channel_sequence: 2,
						data: Uint8Array.of(2),
						kind: "stream.chunk",
						stream_id: "asset_1",
					});

					return yield* stream.pipe(Stream.runCollect, Effect.flip);
				}),
			),
		);

		expect(result).toBeInstanceOf(ArtisanClientError);
		expect(result).toMatchObject({ code: "stream_overflow", retryable: false });
		expect(sent).toEqual([
			{
				connection_id: "connection_1",
				frame: {
					channel_id: "binary_stream_1",
					channel_sequence: 0,
					kind: "stream.bind",
					stream_id: "asset_1",
					stream_ticket: "stream_ticket_1",
				},
				kind: "transport.stream",
				transport_version: 1,
			},
			{
				connection_id: "connection_1",
				frame: {
					channel_id: "binary_stream_1",
					channel_sequence: 1,
					kind: "stream.end",
					reason: "cancelled",
					stream_id: "asset_1",
				},
				kind: "transport.stream",
				transport_version: 1,
			},
		]);
	});
});
