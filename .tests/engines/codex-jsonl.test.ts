import { describe, expect, it } from "vitest";

import { CodexJsonlFramer, CodexJsonlMalformedLineError } from "@artisan/engines";

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
});
