import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeCommandEnvelope,
	DecodeInboundControlEnvelope,
	DecodeOutboundControlEnvelope,
	type ContentIdentity,
	type WorkspaceChange,
	type WorkspaceReplaceApproval,
} from "@artisan/protocol";

const timestamp = "2026-07-11T08:00:00.000Z";
const content_hash = "a".repeat(64);

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

function identity(byte_count = 0): ContentIdentity {
	return {
		algorithm: "sha256",
		byte_count,
		content_hash,
	};
}

function workspace_change(overrides: Partial<WorkspaceChange> = {}): WorkspaceChange {
	return {
		after_identity: identity(),
		agent_id: "agent_1",
		before_identity: identity(),
		change_id: "change_1",
		created_at: timestamp,
		path: "src/main.ts",
		review_state: "needs_review",
		rollback_state: "available",
		run_id: "run_1",
		source_command_id: "command_1",
		thread_id: "thread_1",
		updated_at: timestamp,
		version: 1,
		workspace_id: "workspace_1",
		...overrides,
	};
}

function workspace_change_diff() {
	const patch = "@@ -1,1 +1,1 @@\n-old\n+new\n";

	return {
		added_line_count: 1,
		after_identity: identity(4),
		before_identity: identity(4),
		change_id: "change_1",
		context_lines: 3,
		format: "unified",
		format_version: 1,
		patch,
		patch_identity: {
			algorithm: "sha256" as const,
			byte_count: new TextEncoder().encode(patch).byteLength,
			content_hash,
		},
		path: "src/main.ts",
		removed_line_count: 1,
		thread_id: "thread_1",
		truncated: false,
		workspace_id: "workspace_1",
	};
}

function workspace_replace_approval(
	state: WorkspaceReplaceApproval["state"] = "requested",
): WorkspaceReplaceApproval {
	const approval = {
		after_identity: identity(4),
		agent_id: "agent_1",
		approval_id: "approval_1",
		before_identity: identity(4),
		change_id: "change_1",
		created_at: timestamp,
		path: "src/main.ts",
		policy: "on_request" as const,
		reason: "The replacement updates the workspace fixture.",
		run_id: "run_1",
		thread_id: "thread_1",
		updated_at: timestamp,
		workspace_id: "workspace_1",
	};

	if (state === "requested") {
		return { ...approval, state };
	}

	if (state === "denied") {
		return {
			...approval,
			decided_at: timestamp,
			decision: "denied",
			decision_message_id: "decision_1",
			state,
		};
	}

	return {
		...approval,
		decided_at: timestamp,
		decision: "approved",
		decision_message_id: "decision_1",
		state,
	};
}

describe("workspace changes protocol codec", () => {
	it("roundtrips every workspace-change envelope through the public codecs", async () => {
		const inbound_envelopes = [
			frontend_envelope("workspace.file.read.query", {
				path: "src/main.ts",
				workspace_id: "workspace_1",
			}),
			{
				...frontend_envelope("workspace.file.replace", {
					approval_request: { reason: "Replace the generated workspace fixture." },
					change_id: "change_1",
					content: "export {};\n",
					expected_before: identity(10),
					path: "src/main.ts",
					workspace_id: "workspace_1",
				}),
				agent_id: "agent_1",
				raw_origin: { provider: "codex", reference: "item_1" },
				run_id: "run_1",
				thread_id: "thread_1",
			},
			{
				...frontend_envelope("workspace.change.review", { change_id: "change_1" }),
				thread_id: "thread_1",
			},
			{
				...frontend_envelope("workspace.change.rollback", {
					change_id: "change_1",
					expected_after: identity(10),
				}),
				thread_id: "thread_1",
			},
			frontend_envelope("workspace.change.list.query", {
				thread_id: "thread_1",
				workspace_id: "workspace_1",
			}),
			frontend_envelope("workspace.change.diff.query", {
				change_id: "change_1",
				thread_id: "thread_1",
			}),
			frontend_envelope("workspace.replace.approval.query", {
				approval_id: "approval_1",
				thread_id: "thread_1",
			}),
			{
				...frontend_envelope("workspace.replace.approval.respond", {
					approval_id: "approval_1",
					approved: true,
				}),
				thread_id: "thread_1",
			},
		];
		const outbound_envelopes = [
			{
				correlation_id: "read_1",
				kind: "workspace.file.read.query.result",
				message_id: "read_result_1",
				origin: "backend",
				payload: {
					content: "export {};\n",
					identity: identity(10),
					path: "src/main.ts",
					workspace_id: "workspace_1",
				},
				protocol_version: 1,
				schema_version: 1,
				sent_at: timestamp,
			},
			{
				correlation_id: "list_1",
				kind: "workspace.change.list.query.result",
				message_id: "list_result_1",
				origin: "backend",
				payload: { changes: [workspace_change()], journal_sequence: 1 },
				protocol_version: 1,
				schema_version: 1,
				sent_at: timestamp,
			},
			{
				correlation_id: "diff_1",
				kind: "workspace.change.diff.query.result",
				message_id: "diff_result_1",
				origin: "backend",
				payload: workspace_change_diff(),
				protocol_version: 1,
				schema_version: 1,
				sent_at: timestamp,
			},
			{
				correlation_id: "approval_query_1",
				kind: "workspace.replace.approval.query.result",
				message_id: "approval_query_result_1",
				origin: "backend",
				payload: {
					approval: workspace_replace_approval(),
					diff: workspace_change_diff(),
				},
				protocol_version: 1,
				schema_version: 1,
				sent_at: timestamp,
			},
			{
				causation_id: "approval_1",
				correlation_id: "approval_1",
				journal_sequence: 2,
				kind: "event",
				message_id: "workspace_replace_approval_event_1",
				origin: "backend",
				payload: {
					approval: workspace_replace_approval(),
					type: "workspace.replace.approval.updated",
				},
				protocol_version: 1,
				schema_version: 1,
				sequence: 1,
				sent_at: timestamp,
				stream_id: "thread_1",
				thread_id: "thread_1",
			},
			{
				causation_id: "command_1",
				correlation_id: "replace_1",
				journal_sequence: 2,
				kind: "event",
				message_id: "workspace_change_event_1",
				origin: "backend",
				payload: {
					action: "recorded",
					change: workspace_change(),
					type: "workspace.change.updated",
				},
				protocol_version: 1,
				schema_version: 1,
				sequence: 1,
				sent_at: timestamp,
				stream_id: "thread_1",
				thread_id: "thread_1",
			},
		];

		for (const envelope of inbound_envelopes) {
			await expect(
				Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
			).resolves.toEqual(envelope);
		}

		for (const envelope of outbound_envelopes) {
			await expect(
				Effect.runPromise(DecodeOutboundControlEnvelope(envelope)),
			).resolves.toEqual(envelope);
		}
	});

	it.each([
		"/src/main.ts",
		"C:/repo/src/main.ts",
		"C:src/main.ts",
		"../src/main.ts",
		"src/../main.ts",
		"src\\main.ts",
		"src/\u0000main.ts",
	])("rejects the invalid workspace path %j", async (path) => {
		const envelope = frontend_envelope("workspace.file.read.query", {
			path,
			workspace_id: "workspace_1",
		});

		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
		).rejects.toBeDefined();
	});

	it("rejects workspace text larger than the V1 UTF-8 control-frame bound", async () => {
		const envelope = {
			...frontend_envelope("workspace.file.replace", {
				change_id: "change_1",
				content: "\u{1F600}".repeat(1_048_577),
				expected_before: identity(),
				path: "src/main.ts",
				workspace_id: "workspace_1",
			}),
			agent_id: "agent_1",
			run_id: "run_1",
			thread_id: "thread_1",
		};

		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
		).rejects.toBeDefined();
	});

	it.each(["", "  \n\t ", "reason\u0000", "x".repeat(4097)])(
		"rejects an invalid replacement approval reason %j",
		async (reason) => {
			const envelope = {
				...frontend_envelope("workspace.file.replace", {
					approval_request: { reason },
					change_id: "change_1",
					content: "",
					expected_before: identity(),
					path: "src/main.ts",
					workspace_id: "workspace_1",
				}),
				agent_id: "agent_1",
				run_id: "run_1",
				thread_id: "thread_1",
			};

			await expect(
				Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
			).rejects.toBeDefined();
		},
	);

	it.each(["requested", "approved", "executing", "denied", "applied", "rejected"] as const)(
		"roundtrips the %s workspace replacement approval state",
		async (state) => {
			const approval = workspace_replace_approval(state);
			const envelope = {
				causation_id: "approval_1",
				correlation_id: "approval_1",
				journal_sequence: 2,
				kind: "event",
				message_id: `workspace_replace_approval_${state}`,
				origin: "backend",
				payload: {
					approval,
					type: "workspace.replace.approval.updated",
				},
				protocol_version: 1,
				schema_version: 1,
				sequence: 1,
				sent_at: timestamp,
				stream_id: "thread_1",
				thread_id: "thread_1",
			};

			await expect(
				Effect.runPromise(DecodeOutboundControlEnvelope(envelope)),
			).resolves.toEqual(envelope);
		},
	);

	it("rejects impossible approval lifecycle metadata and source bytes in approval events", async () => {
		const missing_decision = {
			...workspace_replace_approval("applied"),
			decided_at: undefined,
		};
		const requested_with_decision = {
			...workspace_replace_approval("requested"),
			decided_at: timestamp,
			decision: "approved",
			decision_message_id: "decision_1",
		};
		const denied_as_approved = {
			...workspace_replace_approval("denied"),
			decision: "approved",
		};
		const approval_with_patch = {
			...workspace_replace_approval(),
			patch: workspace_change_diff().patch,
		};

		for (const approval of [
			missing_decision,
			requested_with_decision,
			denied_as_approved,
			approval_with_patch,
		]) {
			await expect(
				Effect.runPromise(
					DecodeOutboundControlEnvelope({
						causation_id: "approval_1",
						correlation_id: "approval_1",
						journal_sequence: 2,
						kind: "event",
						message_id: "workspace_replace_approval_invalid",
						origin: "backend",
						payload: {
							approval,
							type: "workspace.replace.approval.updated",
						},
						protocol_version: 1,
						schema_version: 1,
						sequence: 1,
						sent_at: timestamp,
						stream_id: "thread_1",
						thread_id: "thread_1",
					}),
				),
			).rejects.toBeDefined();
		}
	});

	it("rejects malformed content hashes", async () => {
		const envelope = {
			...frontend_envelope("workspace.file.replace", {
				change_id: "change_1",
				content: "",
				expected_before: { ...identity(), content_hash: "A".repeat(64) },
				path: "src/main.ts",
				workspace_id: "workspace_1",
			}),
			agent_id: "agent_1",
			run_id: "run_1",
			thread_id: "thread_1",
		};

		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
		).rejects.toBeDefined();
	});

	it.each(["thread_id", "run_id", "agent_id"] as const)(
		"requires replacement attribution field %s",
		async (field) => {
			const envelope = {
				...frontend_envelope("workspace.file.replace", {
					change_id: "change_1",
					content: "",
					expected_before: identity(),
					path: "src/main.ts",
					workspace_id: "workspace_1",
				}),
				agent_id: "agent_1",
				run_id: "run_1",
				thread_id: "thread_1",
			};

			delete envelope[field];

			await expect(
				Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
			).rejects.toBeDefined();
		},
	);

	it("rejects excess workspace properties and source text in command payloads", async () => {
		const excess_envelope = frontend_envelope("workspace.file.read.query", {
			extra: "nope",
			path: "src/main.ts",
			workspace_id: "workspace_1",
		});
		const command_envelope = {
			...frontend_envelope("command", {
				content: "private source text",
				title: "Workspace change",
				type: "thread.create",
			}),
			thread_id: "thread_1",
		};

		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(excess_envelope)),
		).rejects.toBeDefined();
		await expect(
			Effect.runPromise(DecodeCommandEnvelope(command_envelope)),
		).rejects.toBeDefined();
	});

	it.each([
		{
			...frontend_envelope("workspace.change.diff.query", {
				change_id: "change_1",
				thread_id: "thread with whitespace",
			}),
		},
		{
			...frontend_envelope("workspace.change.diff.query", {
				change_id: "change_1",
				thread_id: "thread_1",
			}),
			unexpected: true,
		},
	])("rejects malformed diff query envelopes", async (envelope) => {
		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
		).rejects.toBeDefined();
	});

	it("rejects a diff result whose patch byte count is inconsistent", async () => {
		const payload = workspace_change_diff();
		payload.patch_identity.byte_count += 1;

		await expect(
			Effect.runPromise(
				DecodeOutboundControlEnvelope({
					correlation_id: "diff_1",
					kind: "workspace.change.diff.query.result",
					message_id: "diff_result_1",
					origin: "backend",
					payload,
					protocol_version: 1,
					schema_version: 1,
					sent_at: timestamp,
				}),
			),
		).rejects.toBeDefined();
	});

	it("rejects an oversized diff patch without expanding the assertion output", async () => {
		const patch = "x".repeat(16 * 1024 * 1024 + 1);
		const payload = {
			...workspace_change_diff(),
			patch,
			patch_identity: {
				...workspace_change_diff().patch_identity,
				byte_count: patch.length,
			},
		};

		await expect(
			Effect.runPromise(
				DecodeOutboundControlEnvelope({
					correlation_id: "diff_1",
					kind: "workspace.change.diff.query.result",
					message_id: "diff_result_1",
					origin: "backend",
					payload,
					protocol_version: 1,
					schema_version: 1,
					sent_at: timestamp,
				}),
			),
		).rejects.toBeDefined();
	});
});
