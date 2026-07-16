import { Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	capability_safe_summary_maximum_bytes,
	capability_identifier_maximum_bytes,
	capability_visible_label_maximum_characters,
	CapabilityInvocationUpdatedEvent,
	EngineNativeActionObservedEvent,
} from "@artisan/protocol";

const decode_capability_invocation = Schema.decodeUnknownSync(CapabilityInvocationUpdatedEvent, {
	onExcessProperty: "error",
});
const encode_capability_invocation = Schema.encodeUnknownSync(CapabilityInvocationUpdatedEvent);
const decode_native_action = Schema.decodeUnknownSync(EngineNativeActionObservedEvent, {
	onExcessProperty: "error",
});
const encode_native_action = Schema.encodeUnknownSync(EngineNativeActionObservedEvent);

describe("capability protocol codec", () => {
	it("strictly roundtrips source-safe capability and native-action events", () => {
		const invocation = {
			effect: "read" as const,
			invocation_id: "tool_1",
			label: "Search workspace",
			source: "artisan" as const,
			state: "completed" as const,
			summary: "Matched 12 files.",
			type: "capability.invocation.updated" as const,
		};
		const native_action = {
			action_id: "native_action_1",
			effect: "unknown" as const,
			label: "Provider search",
			source: "engine" as const,
			state: "observed" as const,
			summary: "Observed without provider output.",
			type: "engine.native_action.observed" as const,
		};

		expect(encode_capability_invocation(decode_capability_invocation(invocation))).toEqual(
			invocation,
		);
		expect(encode_native_action(decode_native_action(native_action))).toEqual(native_action);

		for (const state of [
			"started",
			"progress",
			"approval_required",
			"running",
			"completed",
			"failed",
			"denied",
			"outcome_unknown",
			"suspended",
		] as const) {
			expect(decode_capability_invocation({ ...invocation, state }).state).toBe(state);
		}

		for (const event of [
			{ ...invocation, arguments: { token: "secret" } },
			{ ...invocation, results: ["secret"] },
		]) {
			expect(() => decode_capability_invocation(event)).toThrow();
		}

		for (const event of [
			{ ...native_action, arguments: { query: "private" } },
			{ ...native_action, results: { output: "private" } },
		]) {
			expect(() => decode_native_action(event)).toThrow();
		}
	});

	it("rejects oversized and hidden-control visible text", () => {
		const invocation = {
			effect: "unknown" as const,
			invocation_id: "tool_1",
			label: "Search workspace",
			source: "engine" as const,
			state: "started" as const,
			type: "capability.invocation.updated" as const,
		};

		for (const event of [
			{
				...invocation,
				invocation_id: "x".repeat(capability_identifier_maximum_bytes + 1),
			},
			{ ...invocation, invocation_id: "tool\u0000id" },
			{ ...invocation, label: "x".repeat(capability_visible_label_maximum_characters + 1) },
			{ ...invocation, label: "visible\u0000label" },
			{ ...invocation, label: "\u{1F600}".repeat(129) },
			{
				...invocation,
				summary: "\u{1F600}".repeat(capability_safe_summary_maximum_bytes / 4 + 1),
			},
			{ ...invocation, summary: "visible\u200Bsummary" },
		]) {
			expect(() => decode_capability_invocation(event)).toThrow();
		}
	});
});
