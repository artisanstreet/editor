import { Effect, Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeInboundControlEnvelope,
	DecodeOutboundControlEnvelope,
	DecodeStreamEnvelope,
	EncodeOutboundControlEnvelope,
	EncodeStreamEnvelope,
	StreamEnvelope,
} from "@artisan/protocol";

function make_trace(kind: string, origin: "frontend" | "backend") {
	return {
		kind,
		message_id: "message_1",
		origin,
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T08:00:00.000Z",
	};
}

describe("protocol lifecycle", () => {
	it("decodes a pre-negotiation hello with global and stream resume cursors", async () => {
		const input = {
			kind: "hello",
			message_id: "hello_1",
			origin: "frontend",
			schema_version: 1,
			sent_at: "2026-07-10T08:00:00.000Z",
			payload: {
				last_journal_sequence: 12,
				resume_cursors: [{ stream_id: "thread:thread_1", sequence: 4 }],
				supported_protocol_versions: [1],
			},
		};

		const decoded = await Effect.runPromise(DecodeInboundControlEnvelope(input));

		expect(decoded).toEqual(input);
	});

	it("rejects a hello that includes a negotiated protocol version", async () => {
		const input = {
			kind: "hello",
			message_id: "hello_1",
			origin: "frontend",
			protocol_version: 1,
			schema_version: 1,
			sent_at: "2026-07-10T08:00:00.000Z",
			payload: {
				last_journal_sequence: 0,
				resume_cursors: [],
				supported_protocol_versions: [1],
			},
		};

		await expect(Effect.runPromise(DecodeInboundControlEnvelope(input))).rejects.toBeDefined();
	});

	it("rejects unsupported or unrecognised inbound shapes", async () => {
		const unsupported_version = {
			...make_trace("heartbeat.pong", "frontend"),
			correlation_id: "message_1",
			protocol_version: 2,
			payload: { nonce: "ping_1" },
		};
		const unknown_kind = {
			...make_trace("thread.delete", "frontend"),
			payload: {},
		};

		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(unsupported_version)),
		).rejects.toBeDefined();
		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(unknown_kind)),
		).rejects.toBeDefined();
	});

	it("decodes welcome binding and heartbeat configuration", async () => {
		const welcome = {
			...make_trace("welcome", "backend"),
			correlation_id: "hello_1",
			payload: {
				connection_id: "connection_1",
				current_cursors: [],
				heartbeat_interval_ms: 15_000,
				heartbeat_timeout_ms: 45_000,
				journal_sequence: 12,
				stream_ticket: "stream_ticket_1",
			},
		};

		await expect(Effect.runPromise(DecodeOutboundControlEnvelope(welcome))).resolves.toEqual(
			welcome,
		);
	});

	it("allows a versionless protocol error before negotiation", async () => {
		const error = {
			kind: "protocol.error",
			message_id: "error_1",
			origin: "backend",
			schema_version: 1,
			sent_at: "2026-07-10T08:00:00.000Z",
			payload: {
				code: "protocol.unsupported_version",
				message: "No supported protocol version was offered.",
				retryable: false,
			},
		};

		await expect(Effect.runPromise(DecodeOutboundControlEnvelope(error))).resolves.toEqual(
			error,
		);
	});

	it("decodes thread-list queries and subscription requests", async () => {
		const query = {
			...make_trace("thread.list.query", "frontend"),
			payload: {},
		};
		const subscribe = {
			...make_trace("subscribe", "frontend"),
			payload: { type: "thread.list" },
			subscription_id: "subscription_1",
		};

		await expect(Effect.runPromise(DecodeInboundControlEnvelope(query))).resolves.toEqual(
			query,
		);
		await expect(Effect.runPromise(DecodeInboundControlEnvelope(subscribe))).resolves.toEqual(
			subscribe,
		);
	});

	it("rejects negative global and stream cursor positions", async () => {
		const ack = {
			...make_trace("ack", "frontend"),
			payload: {
				journal_sequence: -1,
				stream_cursors: [{ stream_id: "thread:thread_1", sequence: -1 }],
			},
		};

		await expect(Effect.runPromise(DecodeInboundControlEnvelope(ack))).rejects.toBeDefined();
	});

	it("decodes replay requests with a global journal cursor", async () => {
		const replay = {
			...make_trace("replay", "frontend"),
			payload: {
				after_journal_sequence: 12,
				stream_cursors: [{ stream_id: "thread:thread_1", sequence: 4 }],
			},
		};

		await expect(Effect.runPromise(DecodeInboundControlEnvelope(replay))).resolves.toEqual(
			replay,
		);
	});

	it("encodes backend heartbeat pings and decodes correlated frontend pongs", async () => {
		const ping = {
			...make_trace("heartbeat.ping", "backend"),
			payload: { nonce: "ping_1" },
		};
		const pong = {
			...make_trace("heartbeat.pong", "frontend"),
			correlation_id: "message_1",
			payload: { nonce: "ping_1" },
		};

		await expect(Effect.runPromise(EncodeOutboundControlEnvelope(ping))).resolves.toEqual(ping);
		await expect(Effect.runPromise(DecodeInboundControlEnvelope(pong))).resolves.toEqual(pong);
	});

	it("validates reserved stream frames with portable encoded binary data", async () => {
		const input = {
			channel_id: "channel_1",
			channel_sequence: 3,
			kind: "stream.chunk",
			message_id: "chunk_1",
			origin: "backend",
			protocol_version: 1,
			schema_version: 1,
			sent_at: "2026-07-10T08:00:00.000Z",
			stream_id: "terminal:terminal_1",
			payload: {
				data: "SGVsbG8=",
				encoding: "base64",
			},
		};

		const decoded = await Effect.runPromise(DecodeStreamEnvelope(input));

		expect(decoded).toEqual({
			...input,
			payload: {
				data: new Uint8Array([72, 101, 108, 108, 111]),
				encoding: "base64",
			},
		});
		await expect(Effect.runPromise(EncodeStreamEnvelope(decoded))).resolves.toEqual(input);
	});

	it("validates stream binding, readiness, and end frames", async () => {
		const frames = [
			{
				channel_id: "channel_1",
				channel_sequence: 0,
				kind: "stream.bind",
				message_id: "bind_1",
				origin: "frontend",
				protocol_version: 1,
				schema_version: 1,
				sent_at: "2026-07-10T08:00:00.000Z",
				payload: { stream_id: "terminal:terminal_1" },
			},
			{
				channel_id: "channel_1",
				channel_sequence: 1,
				kind: "stream.ready",
				message_id: "ready_1",
				origin: "backend",
				protocol_version: 1,
				schema_version: 1,
				sent_at: "2026-07-10T08:00:00.000Z",
				payload: { stream_id: "terminal:terminal_1" },
			},
			{
				channel_id: "channel_1",
				channel_sequence: 2,
				kind: "stream.end",
				message_id: "end_1",
				origin: "backend",
				protocol_version: 1,
				schema_version: 1,
				sent_at: "2026-07-10T08:00:00.000Z",
				payload: { reason: "completed" },
				stream_id: "terminal:terminal_1",
			},
		];

		for (const frame of frames) {
			await expect(
				Effect.runPromise(
					Schema.decodeUnknownEffect(StreamEnvelope, { onExcessProperty: "error" })(
						frame,
					),
				),
			).resolves.toEqual(frame);
		}
	});
});
