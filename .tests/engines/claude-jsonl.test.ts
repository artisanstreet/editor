import { describe, expect, it } from "vitest";

import {
	ClaudeJsonlFramer,
	ClaudeJsonlMalformedLineError,
	ClaudeJsonlOversizedLineError,
} from "../../modules/engines/src/claude/jsonl";

describe("Claude CLI JSONL framing", () => {
	it("retains exact JSONL bytes across arbitrary chunks", () => {
		const framer = new ClaudeJsonlFramer({ max_frame_bytes: 128 });
		expect(framer.PushRecovering(new TextEncoder().encode('{"type":"system"'))).toEqual([]);
		const frames = framer.PushRecovering(new TextEncoder().encode("}\n"));
		expect(frames[0]).toMatchObject({
			payload: { type: "system" },
			raw_frame_base64: "eyJ0eXBlIjoic3lzdGVtIn0=",
		});
	});

	it("contains malformed and oversized provider lines", () => {
		const malformed = new ClaudeJsonlFramer({ max_frame_bytes: 8 }).PushRecovering(
			new TextEncoder().encode("nope\n"),
		);
		expect(malformed[0]).toBeInstanceOf(ClaudeJsonlMalformedLineError);
		const oversized = new ClaudeJsonlFramer({ max_frame_bytes: 3 }).PushRecovering(
			new TextEncoder().encode("1234\n"),
		);
		expect(oversized[0]).toBeInstanceOf(ClaudeJsonlOversizedLineError);
	});

	it("treats a trailing unterminated provider record as one bounded record", () => {
		const valid = new ClaudeJsonlFramer({ max_frame_bytes: 128 });
		expect(valid.PushRecovering(new TextEncoder().encode('{"type":"result"}'))).toEqual([]);
		expect(valid.FinishRecovering()[0]).toMatchObject({ payload: { type: "result" } });

		const malformed = new ClaudeJsonlFramer({ max_frame_bytes: 128 });
		malformed.PushRecovering(new TextEncoder().encode("not-json"));
		expect(malformed.FinishRecovering()[0]).toBeInstanceOf(ClaudeJsonlMalformedLineError);
	});

	it("discards an oversized unterminated line through its newline before resynchronizing", () => {
		const framer = new ClaudeJsonlFramer({ max_frame_bytes: 20 });
		expect(framer.PushRecovering(new TextEncoder().encode("x".repeat(21)))[0]).toBeInstanceOf(
			ClaudeJsonlOversizedLineError,
		);
		expect(
			framer.PushRecovering(new TextEncoder().encode('{"tail":true}\n{"fresh":true}\n')),
		).toEqual([expect.objectContaining({ payload: { fresh: true } })]);
	});
});
