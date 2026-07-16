import { Option, Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecideApprovalRequest,
	DecideApprovalResult,
	InvokeRequest,
	InvokeResult,
	ListEligibleRequest,
	ListEligibleResult,
	ToolInvocationProjection,
} from "@artisan/protocol";

const timestamp = "2026-07-16T08:00:00.000Z";

const decode_decide_approval_request = Schema.decodeUnknownSync(DecideApprovalRequest, {
	onExcessProperty: "error",
});
const decode_decide_approval_result = Schema.decodeUnknownSync(DecideApprovalResult, {
	onExcessProperty: "error",
});
const decode_invoke_request = Schema.decodeUnknownSync(InvokeRequest, {
	onExcessProperty: "error",
});
const decode_invoke_request_option = Schema.decodeUnknownOption(InvokeRequest, {
	onExcessProperty: "error",
});
const decode_invoke_result = Schema.decodeUnknownSync(InvokeResult, {
	onExcessProperty: "error",
});
const decode_list_eligible_request = Schema.decodeUnknownSync(ListEligibleRequest, {
	onExcessProperty: "error",
});
const decode_list_eligible_result = Schema.decodeUnknownSync(ListEligibleResult, {
	onExcessProperty: "error",
});
const decode_invocation_projection = Schema.decodeUnknownSync(ToolInvocationProjection, {
	onExcessProperty: "error",
});

const context = {
	agent_id: "agent_1",
	run_id: "run_1",
	thread_id: "thread_1",
	workspace_id: "workspace_1",
} as const;

const public_descriptor = {
	approval_policy: "required",
	effect: "workspace_mutation",
	label: "Apply release note",
	revision: 2,
	source: "artisan",
	summary: "Updates the release note selected by the run.",
	tool_id: "artisan.release_note.apply",
} as const;

const descriptor = {
	...public_descriptor,
	input_schema: {
		additionalProperties: false,
		properties: {
			note_id: { type: "string" },
		},
		required: ["note_id"],
		type: "object",
	},
} as const;

const invoke_request = {
	arguments: {
		metadata: [true, null, 3],
		note_id: "note_1",
	},
	context,
	request_id: "request_1",
	tool: { revision: descriptor.revision, tool_id: descriptor.tool_id },
} as const;

const completed_invocation = {
	approval: {
		approval_id: "approval_1",
		context,
		decided_at: timestamp,
		decision: "approved",
		decision_id: "decision_1",
		invocation_id: "invocation_1",
		request_id: "request_1",
		tool: { revision: descriptor.revision, tool_id: descriptor.tool_id },
	},
	context,
	created_at: timestamp,
	invocation_id: "invocation_1",
	request_id: "request_1",
	settled_at: timestamp,
	state: "completed",
	started_at: timestamp,
	tool: public_descriptor,
	updated_at: timestamp,
} as const;

describe("tool control protocol codec", () => {
	it("roundtrips representative list, invoke, and approval contracts", () => {
		const list_request = { context };
		const list_result = {
			tools: [
				{ descriptor, state: "eligible" },
				{
					descriptor: { ...descriptor, tool_id: "marketplace.release_note.publish" },
					reason_code: "marketplace.account_unavailable",
					state: "unavailable",
				},
			],
		};
		const invoke_result = {
			invocation: completed_invocation,
			outcome: "completed",
			result: { published: true, version: 3 },
		};
		const approval_result = {
			approval: {
				approval_id: "approval_1",
				context,
				created_at: timestamp,
				decided_at: timestamp,
				decision_id: "decision_1",
				invocation_id: "invocation_1",
				request_id: "request_1",
				state: "executing",
				started_at: timestamp,
				tool: public_descriptor,
				updated_at: timestamp,
			},
		};

		expect(decode_list_eligible_request(list_request)).toEqual(list_request);
		expect(decode_list_eligible_result(list_result)).toEqual(list_result);
		expect(decode_invoke_request(invoke_request)).toEqual(invoke_request);
		expect(decode_invoke_result(invoke_result)).toEqual(invoke_result);
		expect(
			decode_decide_approval_request({
				approval_id: "approval_1",
				decision: "approved",
				decision_id: "decision_1",
			}),
		).toEqual({
			approval_id: "approval_1",
			decision: "approved",
			decision_id: "decision_1",
		});
		expect(decode_decide_approval_result(approval_result)).toEqual(approval_result);
	});

	it("rejects excess properties and invalid canonical tool identifiers or revisions", () => {
		for (const value of [
			{ ...invoke_request, unexpected: true },
			{ ...invoke_request, request_id: "request id" },
			{ ...invoke_request, tool: { ...invoke_request.tool, revision: 0 } },
			{ ...invoke_request, tool: { ...invoke_request.tool, tool_id: "tool id" } },
			{ ...invoke_request, context: { ...context, agent_id: "agent id" } },
		]) {
			expect(() => decode_invoke_request(value)).toThrow();
		}

		expect(() =>
			decode_list_eligible_result({
				tools: [{ descriptor: { ...descriptor, source: "engine" }, state: "eligible" }],
			}),
		).toThrow();
		expect(() =>
			decode_list_eligible_result({
				tools: [
					{ descriptor, state: "eligible" },
					{ descriptor: { ...descriptor, revision: 3 }, state: "eligible" },
				],
			}),
		).toThrow();
	});

	it("rejects non-JSON, oversized, deep, and prototype-sensitive private tool data", () => {
		const deep_arguments: { nested?: unknown } = {};
		let current = deep_arguments;

		for (let index = 0; index <= 32; index += 1) {
			current.nested = {};
			current = current.nested as { nested?: unknown };
		}

		const hidden_arguments = { note_id: "note_1" };
		Object.defineProperty(hidden_arguments, "private", { value: "nope" });
		const cyclic_arguments: Record<string, unknown> = { note_id: "note_1" };
		cyclic_arguments.self = cyclic_arguments;
		const custom_array = ["value"];
		Object.setPrototypeOf(custom_array, {});
		const hidden_array = ["value"];
		Object.defineProperty(hidden_array, "private", { value: "nope" });
		const symbol_array = ["value"];
		Object.defineProperty(symbol_array, Symbol("private"), { value: "nope" });
		const sparse_array = Array(1);
		const throwing_to_json = {
			note_id: "note_1",
			toJSON: () => {
				throw new Error("toJSON must not run");
			},
		};

		for (const arguments_value of [
			["not", "an", "object"],
			{ callback: () => undefined },
			{ payload: "x".repeat(64 * 1024) },
			deep_arguments,
			hidden_arguments,
			Object.assign(Object.create({ inherited: true }), { note_id: "note_1" }),
			JSON.parse('{"__proto__":"unsafe"}'),
			{ value: 1n },
			cyclic_arguments,
			{ values: custom_array },
			{ values: hidden_array },
			{ values: symbol_array },
			{ values: sparse_array },
			throwing_to_json,
		]) {
			expect(
				Option.isNone(
					decode_invoke_request_option({
						...invoke_request,
						arguments: arguments_value,
					}),
				),
			).toBe(true);
		}

		for (const input_schema of [
			{ properties: {}, type: "string" },
			{ properties: [], type: "object" },
			{ properties: {}, required: ["value", "value"], type: "object" },
		]) {
			expect(() =>
				decode_list_eligible_result({
					tools: [
						{
							descriptor: { ...descriptor, input_schema },
							state: "eligible",
						},
					],
				}),
			).toThrow();
		}

		expect(() =>
			decode_invoke_result({
				invocation: completed_invocation,
				outcome: "completed",
				result: { payload: "x".repeat(64 * 1024) },
			}),
		).toThrow();

		for (const result of [{ callback: () => undefined }, deep_arguments]) {
			expect(() =>
				decode_invoke_result({
					invocation: completed_invocation,
					outcome: "completed",
					result,
				}),
			).toThrow();
		}
	});

	it("rejects impossible lifecycle combinations and private fields in source-safe projections", () => {
		for (const projection of [
			{ ...completed_invocation, state: "approval_required" },
			{ ...completed_invocation, state: "running" },
			{
				...completed_invocation,
				approval_id: "approval_1",
				arguments: { private: true },
				state: "approval_required",
			},
			{ ...completed_invocation, provider_diagnostics: "private failure" },
			{
				...completed_invocation,
				tool: { ...public_descriptor, approval_policy: "automatic" },
			},
			{
				...completed_invocation,
				approval: {
					...completed_invocation.approval,
					decision_id: "different_decision",
					request_id: "different_request",
				},
			},
			{
				...completed_invocation,
				approval_id: "approval_1",
				settled_at: "2026-07-16T07:59:59.000Z",
			},
		]) {
			expect(() => decode_invocation_projection(projection)).toThrow();
		}

		expect(() =>
			decode_invoke_result({
				invocation: { ...completed_invocation, result: { private: true } },
				outcome: "completed",
				result: { published: true },
			}),
		).toThrow();
		expect(() =>
			decode_invoke_result({
				invocation: { ...completed_invocation, state: "failed" },
				outcome: "completed",
				result: { published: true },
			}),
		).toThrow();
		expect(() =>
			decode_invoke_result({
				invocation: { ...completed_invocation, state: "failed" },
				outcome: "failed",
				result: { private: true },
			}),
		).toThrow();

		expect(() =>
			decode_decide_approval_result({
				approval: {
					approval_id: "approval_1",
					context,
					created_at: timestamp,
					invocation_id: "invocation_1",
					request_id: "request_1",
					state: "approved",
					tool: public_descriptor,
					updated_at: timestamp,
				},
			}),
		).toThrow();
		expect(() =>
			decode_decide_approval_result({
				approval: {
					approval_id: "approval_1",
					context,
					created_at: timestamp,
					decided_at: timestamp,
					decision_id: "decision_1",
					invocation_id: "invocation_1",
					request_id: "request_1",
					settled_at: timestamp,
					state: "settled",
					tool: public_descriptor,
					updated_at: timestamp,
				},
			}),
		).toThrow();
		expect(() =>
			decode_decide_approval_result({
				approval: {
					approval_id: "approval_1",
					arguments: { private: true },
					context,
					created_at: timestamp,
					decided_at: timestamp,
					decision_id: "decision_1",
					invocation_id: "invocation_1",
					provider_diagnostics: "private failure",
					request_id: "request_1",
					state: "approved",
					tool: public_descriptor,
					updated_at: timestamp,
				},
			}),
		).toThrow();
		expect(() =>
			decode_decide_approval_result({
				approval: {
					approval_id: "approval_1",
					context,
					created_at: timestamp,
					invocation_id: "invocation_1",
					request_id: "request_1",
					state: "requested",
					tool: { ...public_descriptor, approval_policy: "automatic" },
					updated_at: timestamp,
				},
			}),
		).toThrow();
	});
});
