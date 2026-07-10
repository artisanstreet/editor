import { describe, expect, it } from "vitest";
import { Effect } from "effect";

import { DecodeCommandEnvelope } from "@artisan/protocol";

function make_input() {
	return {
		protocol_version: 1,
		schema_version: 1,
		kind: "command",
		message_id: "message_1",
		thread_id: "thread_1",
		origin: "frontend",
		sent_at: "2026-07-10T08:00:00.000Z",
		payload: {
			type: "thread.create",
			title: "Backend foundation",
		},
	};
}

describe("protocol codec", () => {
	it("decodes a valid command envelope", async () => {
		const input = make_input();

		const decoded = await Effect.runPromise(DecodeCommandEnvelope(input));

		expect(decoded).toEqual(input);
	});

	it("rejects an unknown command type", async () => {
		const input = {
			...make_input(),
			payload: {
				type: "terminal.launch",
				terminal_id: "terminal_1",
			},
		};

		await expect(Effect.runPromise(DecodeCommandEnvelope(input))).rejects.toBeDefined();
	});

	it("rejects empty or whitespace identifiers", async () => {
		const input = {
			...make_input(),
			message_id: " ",
		};

		await expect(Effect.runPromise(DecodeCommandEnvelope(input))).rejects.toBeDefined();
	});

	it("rejects timestamps outside the wire format", async () => {
		const input = {
			...make_input(),
			sent_at: "not-an-iso-date",
		};

		await expect(Effect.runPromise(DecodeCommandEnvelope(input))).rejects.toBeDefined();
	});

	it("rejects impossible calendar timestamps", async () => {
		const input = {
			...make_input(),
			sent_at: "2026-99-99T99:99:99Z",
		};

		await expect(Effect.runPromise(DecodeCommandEnvelope(input))).rejects.toBeDefined();
	});
});
