import { Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	SurfaceItem,
	surface_identifier_maximum_bytes,
	surface_label_maximum_characters,
	surface_raw_origin_identifier_maximum_bytes,
	surface_summary_maximum_bytes,
} from "@artisan/protocol";

const decode_surface_item = Schema.decodeUnknownSync(SurfaceItem, { onExcessProperty: "error" });

const item = {
	group: "Capabilities" as const,
	kind: "capability" as const,
	label: "Capability",
	raw_origin: { provider: "engine_1", reference: "opaque_1" },
	source: "engine" as const,
	state: "observed",
	summary: "Capability observed.",
	surface_id: "surface:capability:capability_1",
	thread_id: "thread_1",
	timestamp: "2026-07-16T12:00:00.000Z",
};

const usage_item = {
	...item,
	group: "Work" as const,
	kind: "run" as const,
	label: "Run",
	source: "artisan" as const,
	state: "updated",
	summary: "Run usage updated.",
	surface_id: "surface:run:run_1",
	usage: { input_tokens: 1_024, output_tokens: 256 },
};

describe("surface protocol", () => {
	it("strictly decodes source-safe canonical items", () => {
		expect(decode_surface_item(item)).toEqual(item);
	});

	it("strictly decodes bounded provider-neutral usage", () => {
		expect(decode_surface_item(usage_item)).toEqual(usage_item);
		expect(
			decode_surface_item({
				...usage_item,
				usage: { input_tokens: 0, output_tokens: Number.MAX_SAFE_INTEGER },
			}),
		).toMatchObject({
			usage: { input_tokens: 0, output_tokens: Number.MAX_SAFE_INTEGER },
		});
	});

	it("rejects excess payloads and unsafe or oversized surface fields", () => {
		for (const value of [
			{ ...item, arguments: { token: "private" } },
			{ ...item, results: { output: "private" } },
			{ ...item, diagnostics: "private" },
			{ ...item, secrets: "private" },
			{ ...item, agent_id: "agent\u200Bid" },
			{ ...item, label: "x".repeat(surface_label_maximum_characters + 1) },
			{ ...item, surface_id: "x".repeat(surface_identifier_maximum_bytes + 1) },
			{ ...item, summary: "x".repeat(surface_summary_maximum_bytes + 1) },
			{ ...item, summary: "hidden\u0000value" },
			{ ...usage_item, usage: { input_tokens: -1, output_tokens: 256 } },
			{ ...usage_item, usage: { input_tokens: 1_024.5, output_tokens: 256 } },
			{
				...usage_item,
				usage: { input_tokens: Number.MAX_SAFE_INTEGER + 1, output_tokens: 256 },
			},
			{
				...usage_item,
				usage: { cached_tokens: 12, input_tokens: 1_024, output_tokens: 256 },
			},
			{
				...item,
				raw_origin: {
					provider: "x".repeat(surface_raw_origin_identifier_maximum_bytes + 1),
					reference: "opaque_1",
				},
			},
			{
				...item,
				raw_origin: { provider: "engine_1", reference: "opaque\u200B1" },
			},
			{
				...item,
				raw_origin: {
					metadata: { private: true },
					provider: "engine_1",
					reference: "opaque_1",
				},
			},
		]) {
			expect(() => decode_surface_item(value)).toThrow();
		}
	});
});
