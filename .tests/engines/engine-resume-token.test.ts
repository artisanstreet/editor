import { Schema } from "effect";
import { describe, expect, it } from "vitest";

import { EngineResumeToken } from "@artisan/engines";

const decode_resume_token = Schema.decodeUnknownSync(EngineResumeToken, {
	onExcessProperty: "error",
});

describe("EngineResumeToken", () => {
	it.each([
		{ native_thread_id: "0197d1d7-25ba-7c1d-9d90-9c30b5c8dd21" },
		{
			native_thread_id: "thread_01J9FMEQPF4R9V4T26Z2C3R4B5",
			opaque_checkpoint: '{"cursor":"provider-owned"}',
		},
	])("decodes bounded provider-native state", (token) => {
		expect(decode_resume_token(token)).toEqual(token);
	});

	it.each([
		{},
		{ native_thread_id: "" },
		{ native_thread_id: " thread_123" },
		{ native_thread_id: "thread\n123" },
		{ native_thread_id: 123 },
		{ native_thread_id: "x".repeat(513) },
		{ native_thread_id: "thread_123", opaque_checkpoint: 123 },
		{ native_thread_id: "thread_123", opaque_checkpoint: "x".repeat(16_385) },
		{ native_thread_id: "thread_123", provider_state: "invented" },
	])("rejects malformed or unbounded persisted state", (token) => {
		expect(() => decode_resume_token(token)).toThrow();
	});
});
