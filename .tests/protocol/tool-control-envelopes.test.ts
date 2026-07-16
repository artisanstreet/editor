import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeInboundControlEnvelope,
	DecodeOutboundControlEnvelope,
	EncodeOutboundControlEnvelope,
} from "@artisan/protocol";

const timestamp = "2026-07-16T12:00:00.000Z";

const public_tool = {
	approval_policy: "required",
	effect: "workspace_mutation",
	label: "Apply release note",
	revision: 2,
	source: "artisan",
	summary: "Updates the release note selected by the run.",
	tool_id: "artisan.release_note.apply",
} as const;

const context = {
	agent_id: "agent_1",
	run_id: "run_1",
	thread_id: "thread_1",
	workspace_id: "workspace_1",
} as const;

const pending_approval = {
	approval_id: "approval_1",
	context,
	created_at: timestamp,
	invocation_id: "invocation_1",
	request_id: "request_1",
	state: "requested",
	tool: public_tool,
	updated_at: timestamp,
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
		tool: { revision: public_tool.revision, tool_id: public_tool.tool_id },
	},
	context,
	created_at: timestamp,
	invocation_id: "invocation_1",
	request_id: "request_1",
	settled_at: timestamp,
	started_at: timestamp,
	state: "completed",
	tool: public_tool,
	updated_at: timestamp,
} as const;

function frontend_envelope(kind: string, payload: unknown) {
	return {
		kind,
		message_id: `message_${kind}`,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: timestamp,
	};
}

function backend_envelope(kind: string, correlation_id: string, payload: unknown) {
	return {
		correlation_id,
		kind,
		message_id: `result_${kind}`,
		origin: "backend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: timestamp,
	};
}

describe("tool control renderer envelopes", () => {
	it("roundtrips source-safe queries, exact-replay decisions, and correlated results", async () => {
		const inbound = [
			frontend_envelope("tool.invocation.query", {
				invocation_id: "invocation_1",
				thread_id: "thread_1",
			}),
			frontend_envelope("tool.approval.query", {
				approval_id: "approval_1",
				thread_id: "thread_1",
			}),
			frontend_envelope("tool.approval.decide", {
				approval_id: "approval_1",
				decision: "approved",
				decision_id: "decision_1",
				thread_id: "thread_1",
			}),
		];
		const outbound = [
			backend_envelope("tool.invocation.query.result", "message_tool.invocation.query", {
				invocation: completed_invocation,
			}),
			backend_envelope("tool.approval.query.result", "message_tool.approval.query", {
				approval: pending_approval,
			}),
			backend_envelope("tool.approval.decide.result", "message_tool.approval.decide", {
				approval: pending_approval,
			}),
		];

		for (const envelope of inbound) {
			await expect(
				Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
			).resolves.toEqual(envelope);
		}

		for (const envelope of outbound) {
			await expect(
				Effect.runPromise(DecodeOutboundControlEnvelope(envelope)),
			).resolves.toEqual(envelope);
			await expect(
				Effect.runPromise(EncodeOutboundControlEnvelope(envelope)),
			).resolves.toEqual(envelope);
		}
	});

	it("rejects malformed and excess renderer envelope input", async () => {
		const malformed_query = frontend_envelope("tool.invocation.query", {
			thread_id: "thread_1",
		});
		const excess_decision = {
			...frontend_envelope("tool.approval.decide", {
				approval_id: "approval_1",
				decision: "approved",
				decision_id: "decision_1",
				thread_id: "thread_1",
			}),
			thread_id: "thread_1",
		};
		const excess_result = backend_envelope(
			"tool.approval.query.result",
			"message_tool.approval.query",
			{ approval: pending_approval, provider_diagnostics: "private" },
		);

		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(malformed_query)),
		).rejects.toThrow();
		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(excess_decision)),
		).rejects.toThrow();
		await expect(
			Effect.runPromise(DecodeOutboundControlEnvelope(excess_result)),
		).rejects.toThrow();
	});

	it("cannot encode completed private tool results through renderer envelopes", async () => {
		const private_result = backend_envelope(
			"tool.invocation.query.result",
			"message_tool.invocation.query",
			{
				invocation: { ...completed_invocation, result: { provider_output: "private" } },
			},
		);
		const encoded = await Effect.runPromise(EncodeOutboundControlEnvelope(private_result));

		expect(encoded).toEqual(
			backend_envelope("tool.invocation.query.result", "message_tool.invocation.query", {
				invocation: completed_invocation,
			}),
		);
		await expect(
			Effect.runPromise(DecodeOutboundControlEnvelope(private_result)),
		).rejects.toThrow();
	});
});
