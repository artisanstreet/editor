import { Buffer } from "node:buffer";

import { describe, expect, it } from "vitest";

import {
	CodexJsonlFramer,
	CodexJsonlMalformedLineError,
	CodexJsonlOversizedLineError,
} from "@artisan/engines";

const encoder = new TextEncoder();
const snowman = String.fromCodePoint(0x2603);

describe("Codex JSONL framing", () => {
	it("preserves fragmented UTF-8 sequences until a complete line arrives", () => {
		const framer = new CodexJsonlFramer();
		const encoded = encoder.encode(`{"message":"snowman: ${snowman}"}\n`);
		const split = encoded.indexOf(0xe2) + 1;

		expect(framer.Push(encoded.subarray(0, split))).toEqual([]);
		expect(framer.Push(encoded.subarray(split))).toEqual([{ message: `snowman: ${snowman}` }]);
	});

	it("emits every coalesced JSONL message in order", () => {
		const framer = new CodexJsonlFramer();

		expect(framer.Push(encoder.encode('{"id":1}\n{"id":2}\n'))).toEqual([{ id: 1 }, { id: 2 }]);
	});

	it("reports malformed lines as a typed failure", () => {
		const framer = new CodexJsonlFramer();

		expect(() => framer.Push(encoder.encode("not json\n"))).toThrow(
			CodexJsonlMalformedLineError,
		);
	});

	it("bounds fragmented oversized lines and recovers at the next frame", () => {
		const framer = new CodexJsonlFramer({ max_frame_bytes: 16 });

		expect(framer.PushRecovering(encoder.encode('{"payload":"'))).toEqual([]);

		const decoded = framer.PushRecovering(encoder.encode('xxxxxxxxxxxxxxxx"}\n{"ok":true}\n'));

		expect(decoded[0]).toBeInstanceOf(CodexJsonlOversizedLineError);
		expect(decoded[0]).toMatchObject({ max_frame_bytes: 16, size_bytes: 30 });
		expect(decoded[1]).toMatchObject({ payload: { ok: true } });
	});

	it("reports a newline-free oversized line as soon as it crosses the bound", () => {
		const framer = new CodexJsonlFramer({ max_frame_bytes: 16 });

		const oversized = framer.PushRecovering(encoder.encode("xxxxxxxxxxxxxxxxxxxx"));

		expect(oversized).toHaveLength(1);
		expect(oversized[0]).toMatchObject({ max_frame_bytes: 16, size_bytes: 20 });
		expect(framer.PushRecovering(encoder.encode("more bytes"))).toEqual([]);
		expect(framer.PushRecovering(encoder.encode('\n{"ok":true}\n'))).toMatchObject([
			{ payload: { ok: true } },
		]);
	});

	it("owns fragmented input and remains correct under one-byte chunking", () => {
		const framer = new CodexJsonlFramer();
		const first = encoder.encode('{"message":"before mutation"');

		expect(framer.Push(first)).toEqual([]);
		first.fill(120);

		const values = [
			...framer.Push(new Uint8Array([125])),
			...framer.Push(new Uint8Array([10])),
		];

		expect(values).toEqual([{ message: "before mutation" }]);
		expect(framer.Finish()).toEqual([]);
	});

	it("decodes a large frame delivered one byte at a time", () => {
		const framer = new CodexJsonlFramer({ max_frame_bytes: 64 * 1_024 });
		const frame = encoder.encode(`${JSON.stringify({ text: "x".repeat(32 * 1_024) })}\n`);
		const values: Array<unknown> = [];

		for (const byte of frame) {
			values.push(...framer.Push(Uint8Array.of(byte)));
		}

		expect(values).toEqual([{ text: "x".repeat(32 * 1_024) }]);
		expect(framer.Finish()).toEqual([]);
	});

	it("caps oversized diagnostics to one per line and retains at most 256 prefix bytes", () => {
		const framer = new CodexJsonlFramer({ max_frame_bytes: 512 });
		const values = framer.PushRecovering(
			encoder.encode(`${"a".repeat(2_048)}\n${"b".repeat(2_048)}\n`),
		);
		const oversized = values.filter((value) => value instanceof CodexJsonlOversizedLineError);

		expect(oversized).toHaveLength(2);
		expect(oversized.map((value) => Buffer.from(value.prefix_base64, "base64").length)).toEqual(
			[256, 256],
		);
	});
});
