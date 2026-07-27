import { MessageChannel } from "node:worker_threads";

import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { make_message_port_like } from "@artisan/transport";
import { adapt_node_message_port } from "@artisan/transport/node";

describe("MessagePort adapters", () => {
	it("round-trips structured clone bytes and rejects post-close sends", async () => {
		const output = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const channel = new MessageChannel();
					const sender = yield* adapt_node_message_port(channel.port1);
					const receiver = yield* adapt_node_message_port(channel.port2);

					yield* sender.Send({
						bytes: Uint8Array.of(1, 2, 3),
						label: "path with spaces",
					});
					const received = yield* receiver.Receive;

					yield* sender.Close;

					const close = yield* sender.Closed;
					const send_error = yield* sender.Send("too late").pipe(Effect.flip);

					return { close, received, send_error };
				}),
			),
		);

		expect(output.received).toEqual({
			bytes: Uint8Array.of(1, 2, 3),
			label: "path with spaces",
		});
		expect(output.close).toEqual({ code: "closed", dropped_messages: 0 });
		expect(output.send_error).toMatchObject({ code: "closed", dropped_messages: 0 });
	});

	it("passes Node transfer lists directly and detaches the sender buffer", async () => {
		const output = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const channel = new MessageChannel();
					const sender = yield* adapt_node_message_port(channel.port1);
					const receiver = yield* adapt_node_message_port(channel.port2);
					const buffer = new ArrayBuffer(4);
					const bytes = new Uint8Array(buffer);

					bytes.set([7, 8, 9, 10]);
					yield* sender.Send(bytes, [buffer]);

					const detached_buffer_bytes = buffer.byteLength;
					const detached_view_bytes = bytes.byteLength;
					const received = yield* receiver.Receive;

					if (!(received instanceof Uint8Array)) {
						return yield* Effect.die("expected transferred Uint8Array");
					}

					return {
						detached_buffer_bytes,
						detached_view_bytes,
						received: [...received],
					};
				}),
			),
		);

		expect(output).toEqual({
			detached_buffer_bytes: 0,
			detached_view_bytes: 0,
			received: [7, 8, 9, 10],
		});
	});

	it("closes with an explicit gap when the native callback buffer overflows", async () => {
		const closed = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const channel = new MessageChannel();
					const sender = yield* adapt_node_message_port(channel.port1);
					const receiver = yield* adapt_node_message_port(channel.port2, {
						incoming_capacity: 1,
					});

					yield* sender.Send("first");
					yield* sender.Send("second");

					return yield* receiver.Closed;
				}),
			),
		);

		expect(closed).toEqual({ code: "overflow", dropped_messages: 1 });
	});

	it("validates buffer limits before registering native listeners", async () => {
		let listener_registrations = 0;
		const failure = await Effect.runPromise(
			Effect.scoped(
				make_message_port_like(
					{
						add_close_listener: () => {
							listener_registrations += 1;

							return () => undefined;
						},
						add_message_error_listener: () => {
							listener_registrations += 1;

							return () => undefined;
						},
						add_message_listener: () => {
							listener_registrations += 1;

							return () => undefined;
						},
						close: () => undefined,
						post_message: () => undefined,
						start: () => undefined,
					},
					{ incoming_capacity: 0 },
				).pipe(Effect.flip),
			),
		);

		expect(failure).toMatchObject({ code: "configuration" });
		expect(listener_registrations).toBe(0);
	});

	it("unwinds partial listener registration when a native hook throws", async () => {
		let closes = 0;
		let removals = 0;
		const failure = await Effect.runPromise(
			Effect.scoped(
				make_message_port_like({
					add_close_listener: () => {
						throw new Error("close listener registration failed");
					},
					add_message_error_listener: () => () => {
						removals += 1;
					},
					add_message_listener: () => () => {
						removals += 1;
					},
					close: () => {
						closes += 1;
					},
					post_message: () => undefined,
					start: () => undefined,
				}).pipe(Effect.flip),
			),
		);

		expect(failure).toMatchObject({ code: "message_error" });
		expect(removals).toBe(1);
		expect(closes).toBe(1);
	});
});
