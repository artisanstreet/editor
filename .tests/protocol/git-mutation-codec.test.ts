import { Effect, Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeInboundControlEnvelope,
	DecodeOutboundControlEnvelope,
	summarize_workspace_git_mutation,
	WorkspaceGitMutationApproval,
	WorkspaceGitMutationApprovalUpdatedEvent,
	WorkspaceGitMutationRequest,
} from "@artisan/protocol";

const timestamp = "2026-07-13T08:00:00.000Z";
const head = "a".repeat(40);

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

const decode_request = Schema.decodeUnknownSync(WorkspaceGitMutationRequest, {
	onExcessProperty: "error",
});
const decode_approval = Schema.decodeUnknownSync(WorkspaceGitMutationApproval, {
	onExcessProperty: "error",
});
const decode_event = Schema.decodeUnknownSync(WorkspaceGitMutationApprovalUpdatedEvent, {
	onExcessProperty: "error",
});
const encode_event = Schema.encodeUnknownSync(WorkspaceGitMutationApprovalUpdatedEvent);

function request(operation: unknown) {
	const continuation =
		typeof operation === "object" &&
		operation !== null &&
		"action" in operation &&
		operation.action !== "start";

	return {
		...(continuation ? { action_approval_id: "approval_conflict" } : {}),
		expected_session_version: 2,
		operation,
		workspace_id: "workspace_1",
	};
}

function approval(state: string, operation: unknown = { type: "commit" }) {
	const base = {
		approval_id: "approval_1",
		created_at: timestamp,
		expected_session_version: 2,
		operation,
		source_branch: "main",
		source_command_id: "command_1",
		source_head: head,
		thread_id: "thread_1",
		updated_at: timestamp,
		workspace_id: "workspace_1",
	};

	if (state === "requested") {
		return { ...base, state };
	}

	if (state === "denied") {
		return {
			...base,
			decided_at: timestamp,
			decision: "denied",
			decision_message_id: "decision_1",
			state,
		};
	}

	if (state === "applied") {
		return {
			...base,
			decided_at: timestamp,
			decision: "approved",
			decision_message_id: "decision_1",
			resulting_head: head,
			state,
		};
	}

	return {
		...base,
		decided_at: timestamp,
		decision: "approved",
		decision_message_id: "decision_1",
		state,
	};
}

describe("Git mutation protocol codec", () => {
	it("roundtrips mutation request, approval, and public query envelopes", async () => {
		const request_envelope = {
			...frontend_envelope(
				"workspace.git.mutation.request",
				request({
					message: "Ship the guarded mutation protocol",
					type: "commit",
				}),
			),
			thread_id: "thread_1",
		};
		const query_envelope = frontend_envelope("workspace.git.mutation.approval.query", {
			approval_id: "approval_1",
			thread_id: "thread_1",
		});
		const respond_envelope = {
			...frontend_envelope("workspace.git.mutation.approval.respond", {
				approval_id: "approval_1",
				approved: true,
			}),
			thread_id: "thread_1",
		};
		const result_envelope = {
			correlation_id: "query_1",
			kind: "workspace.git.mutation.approval.query.result",
			message_id: "query_result_1",
			origin: "backend",
			payload: { approval: approval("requested", { type: "commit" }) },
			protocol_version: 1,
			schema_version: 1,
			sent_at: timestamp,
		};

		for (const envelope of [request_envelope, query_envelope, respond_envelope]) {
			await expect(
				Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
			).resolves.toEqual(envelope);
		}

		await expect(
			Effect.runPromise(DecodeOutboundControlEnvelope(result_envelope)),
		).resolves.toEqual(result_envelope);
	});

	it("rejects private commit text and excess diagnostics in public query results", async () => {
		const result_envelope = {
			correlation_id: "query_1",
			kind: "workspace.git.mutation.approval.query.result",
			message_id: "query_result_1",
			origin: "backend",
			payload: { approval: approval("requested", { type: "commit" }) },
			protocol_version: 1,
			schema_version: 1,
			sent_at: timestamp,
		};

		await expect(
			Effect.runPromise(
				DecodeOutboundControlEnvelope({
					...result_envelope,
					payload: {
						approval: approval("requested", {
							message: "private commit text",
							type: "commit",
						}),
					},
				}),
			),
		).rejects.toBeDefined();

		await expect(
			Effect.runPromise(
				DecodeOutboundControlEnvelope({
					...result_envelope,
					payload: {
						approval: {
							...approval("requested", { type: "commit" }),
							stderr: "private diagnostics",
						},
					},
				}),
			),
		).rejects.toBeDefined();
	});

	it.each([
		{ branch: "feature", type: "branch_create" },
		{ target_branch: "feature", type: "checkout" },
		{ mode: "hard", target: head, type: "reset" },
		{ type: "clean" },
		{ message: "Ship the guarded mutation protocol", type: "commit" },
		{ action: "start", target_branch: "feature", type: "merge" },
		{ action: "continue", type: "merge" },
		{ action: "abort", type: "merge" },
		{ action: "start", target_branch: "feature", type: "rebase" },
		{ action: "continue", type: "rebase" },
		{ action: "abort", type: "rebase" },
		{ action: "skip", type: "rebase" },
		{ type: "pull_ff_only" },
		{ remote: "origin", set_upstream: true, target_branch: "feature", type: "push" },
	])("decodes the %s operation", (operation) => {
		expect(decode_request(request(operation))).toMatchObject({ operation });
	});

	it("keeps commit text in the request and out of public approval events", () => {
		const operation = decode_request(
			request({ message: "private commit text", type: "commit" }),
		).operation;
		const event = {
			approval: approval("applied", summarize_workspace_git_mutation(operation)),
			type: "workspace.git.mutation.approval.updated",
		};

		const encoded = encode_event(decode_event(event));

		expect(encoded).toEqual(event);
		expect(encoded.approval.operation).toEqual({ type: "commit" });
		expect(() =>
			decode_event({
				...event,
				approval: approval("applied", {
					message: "private commit text",
					type: "commit",
				}),
			}),
		).toThrow();
	});

	it("binds every continuation to one exact action-required approval", () => {
		expect(() =>
			decode_request({
				expected_session_version: 2,
				operation: { action: "continue", type: "rebase" },
				workspace_id: "workspace_1",
			}),
		).toThrow();
		expect(() =>
			decode_request({
				action_approval_id: "approval_conflict",
				expected_session_version: 2,
				operation: { target_branch: "feature", type: "checkout" },
				workspace_id: "workspace_1",
			}),
		).toThrow();

		expect(decode_request(request({ action: "abort", type: "merge" }))).toMatchObject({
			action_approval_id: "approval_conflict",
		});
	});

	it.each([
		{ operation: { branch: "--danger", type: "branch_create" } },
		{ operation: { target_branch: "feature..unsafe", type: "checkout" } },
		{
			operation: {
				remote: "origin;rm",
				set_upstream: false,
				target_branch: "feature",
				type: "push",
			},
		},
		{ operation: { message: "unsafe\0message", type: "commit" } },
		{ operation: { message: " \n\t ", type: "commit" } },
		{ operation: { message: "x".repeat(4097), type: "commit" } },
		{ operation: { path: "src/private.ts", type: "clean" } },
		{ operation: { type: "unknown" } },
	])("rejects unsafe or excess request intent", ({ operation }) => {
		expect(() => decode_request(request(operation))).toThrow();
	});

	it("enforces exact fields for terminal approvals", () => {
		expect(() => decode_approval(approval("action_required"))).toThrow();
		expect(() => decode_approval(approval("rejected"))).toThrow();
		expect(() => decode_approval(approval("outcome_unknown"))).toThrow();
		expect(() => decode_approval({ ...approval("denied"), decision: "approved" })).toThrow();

		expect(() =>
			decode_approval({ ...approval("action_required"), action: "merge_conflict" }),
		).not.toThrow();
		expect(() =>
			decode_approval({ ...approval("rejected"), reason: "branch_missing" }),
		).not.toThrow();
		expect(() =>
			decode_approval({ ...approval("outcome_unknown"), reason: "verification_failed" }),
		).not.toThrow();
		expect(() =>
			decode_approval({
				...approval("applied"),
				resulting_branch: "main",
				resulting_head: head,
				remote_head: head,
			}),
		).not.toThrow();
	});

	it("rejects unknown lifecycle tags and excess public diagnostics", () => {
		expect(() => decode_approval({ ...approval("requested"), state: "finished" })).toThrow();
		expect(() =>
			decode_event({
				approval: { ...approval("rejected"), reason: "branch_missing", stderr: "private" },
				type: "workspace.git.mutation.approval.updated",
			}),
		).toThrow();
	});
});
