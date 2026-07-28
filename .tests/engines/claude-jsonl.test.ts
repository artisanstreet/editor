import { describe, expect, it } from "vitest";

import { ClaudeJsonlFramer, ClaudeJsonlOversizedLineError } from "@artisan/engines";

describe("Claude JSONL framing", () => {
	it("decodes a multibyte frame split into many tiny chunks", () => {
		const framer = new ClaudeJsonlFramer({ max_frame_bytes: 128 });
		const bytes = new TextEncoder().encode('{"text":"café"}\n');
		const values = Array.from(bytes).flatMap((byte) =>
			framer.PushRecovering(new Uint8Array([byte])),
		);

		expect(values).toHaveLength(1);
		expect(values[0]).toMatchObject({ payload: { text: "café" } });
	});

	it("discards an oversized line until newline and caps raw prefix retention", () => {
		const framer = new ClaudeJsonlFramer({ max_frame_bytes: 64 });
		const values = framer.PushRecovering(
			new TextEncoder().encode(`${"x".repeat(10_000)}\n{"ok":true}\n`),
		);

		expect(values[0]).toBeInstanceOf(ClaudeJsonlOversizedLineError);
		expect(
			(values[0] as ClaudeJsonlOversizedLineError).prefix_base64.length,
		).toBeLessThanOrEqual((256 * 4) / 3 + 4);
		expect(values[1]).toMatchObject({ payload: { ok: true } });
	});
});
