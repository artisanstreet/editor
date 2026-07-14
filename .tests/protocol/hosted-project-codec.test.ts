import { Effect, Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeInboundControlEnvelope,
	DecodeOutboundControlEnvelope,
	HostedProjectCloneApproval,
	HostedProjectCloneRequest,
} from "@artisan/protocol";

const timestamp = "2026-07-14T10:30:00.000Z";

const decode_approval = Schema.decodeUnknownSync(HostedProjectCloneApproval, {
	onExcessProperty: "error",
});
const decode_request = Schema.decodeUnknownSync(HostedProjectCloneRequest, {
	onExcessProperty: "error",
});

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

function repository() {
	return {
		archived: false,
		clone_url: "git@github.com:artisan-editor/protocol.git",
		default_branch: { _tag: "known", name: "main" },
		identity: {
			host: "github.com",
			name: "protocol",
			owner: "artisan-editor",
			provider_id: "github",
		},
		origin: {
			native_id: "R_kgDOK7Y9aA",
			provider_id: "github",
			resource_kind: "repository",
		},
		viewer_permission: "write",
		visibility: "private",
		web_url: "https://github.com/artisan-editor/protocol",
	};
}

function request() {
	return {
		destination_path: "C:\\Projects\\artisan-protocol",
		repository: repository(),
		selection: {
			account_login: "artisan-maintainer",
			host: "github.com",
			provider_id: "github",
		},
	};
}

function approval(state: string) {
	const base = {
		approval_id: "approval_1",
		created_at: timestamp,
		destination_path: "C:\\Projects\\artisan-protocol",
		repository: {
			host: "github.com",
			name: "protocol",
			owner: "artisan-editor",
			provider_id: "github",
			selected_account_login: "artisan-maintainer",
			web_url: "https://github.com/artisan-editor/protocol",
		},
		source_command_id: "command_1",
		thread_id: "thread_1",
		updated_at: timestamp,
	};
	const decision = {
		decided_at: timestamp,
		decision: state === "denied" ? "denied" : "approved",
		decision_message_id: "decision_1",
	};
	const project = {
		display_name: "Artisan Protocol",
		project_id: "project_1",
		root_path: "C:\\Projects\\artisan-protocol",
	};

	if (state === "requested") {
		return { ...base, state };
	}

	if (state === "reused") {
		return { ...base, attachment: "already_attached", project, state };
	}

	if (state === "applied") {
		return { ...base, ...decision, attachment: "attached", project, state };
	}

	if (state === "attachment_conflict") {
		return { ...base, ...decision, project, state };
	}

	if (state === "rejected") {
		return { ...base, ...decision, reason: "provider_unavailable", state };
	}

	if (state === "outcome_unknown") {
		return { ...base, ...decision, reason: "verification_failed", state };
	}

	return state === "denied"
		? { ...base, ...decision, state }
		: { ...base, ...decision, decision: "approved", state };
}

describe("Hosted project clone protocol codec", () => {
	it("roundtrips clone request, approval commands, results, and replay events", async () => {
		const inbound = [
			{
				...frontend_envelope("hosted.project.clone.request", request()),
				thread_id: "thread_1",
			},
			frontend_envelope("hosted.project.clone.approval.query", {
				approval_id: "approval_1",
				thread_id: "thread_1",
			}),
			{
				...frontend_envelope("hosted.project.clone.approval.respond", {
					approval_id: "approval_1",
					approved: true,
				}),
				thread_id: "thread_1",
			},
		];
		const result = {
			correlation_id: "approval_1",
			kind: "hosted.project.clone.approval.query.result",
			message_id: "approval_result_1",
			origin: "backend",
			payload: { approval: approval("requested") },
			protocol_version: 1,
			schema_version: 1,
			sent_at: timestamp,
		};

		for (const envelope of inbound) {
			await expect(
				Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
			).resolves.toEqual(envelope);
		}

		await expect(Effect.runPromise(DecodeOutboundControlEnvelope(result))).resolves.toEqual(
			result,
		);

		for (const state of [
			"requested",
			"reused",
			"approved",
			"executing",
			"applied",
			"attachment_conflict",
			"rejected",
			"outcome_unknown",
			"denied",
		]) {
			const event = {
				causation_id: "clone_1",
				correlation_id: "clone_1",
				journal_sequence: 1,
				kind: "event",
				message_id: `approval_${state}`,
				origin: "backend",
				payload: {
					approval: approval(state),
					type: "hosted.project.clone.approval.updated",
				},
				protocol_version: 1,
				schema_version: 1,
				sequence: 1,
				sent_at: timestamp,
				stream_id: "thread_1",
				thread_id: "thread_1",
			};

			await expect(Effect.runPromise(DecodeOutboundControlEnvelope(event))).resolves.toEqual(
				event,
			);
		}
	});

	it.each([
		{ destination_path: "relative/project" },
		{ destination_path: "C:\\Projects\nproject" },
		{ repository: { ...repository(), clone_url: "https://user:secret@github.com/a/b" } },
		{ repository: { ...repository(), web_url: "ssh://github.com/artisan-editor/protocol" } },
		{
			repository: {
				...repository(),
				clone_url: "https://gitlab.com/artisan-editor/protocol.git",
			},
		},
		{
			repository: {
				...repository(),
				origin: { ...repository().origin, provider_id: "gitlab" },
			},
		},
		{
			repository: {
				...repository(),
				identity: { ...repository().identity, host: "https://github.com" },
			},
		},
		{
			selection: { ...request().selection, host: "github.example.com" },
		},
		{
			selection: { ...request().selection, provider_id: "gitlab" },
		},
	])("rejects malformed clone preparation input", (invalid) => {
		expect(() => decode_request({ ...request(), ...invalid })).toThrow();
	});

	it("rejects excess and private repository fields from public approvals", () => {
		for (const private_field of [
			"provider_raw_output",
			"receipt",
			"file_identity",
			"preparation",
		]) {
			expect(() =>
				decode_approval({ ...approval("requested"), [private_field]: "private" }),
			).toThrow();
		}

		expect(() =>
			decode_approval({
				...approval("requested"),
				repository: {
					...approval("requested").repository,
					clone_url: repository().clone_url,
				},
			}),
		).toThrow();
		expect(() =>
			decode_approval({
				...approval("requested"),
				repository: {
					...approval("requested").repository,
					native_id: repository().origin.native_id,
				},
			}),
		).toThrow();
		expect(() =>
			decode_approval({
				...approval("requested"),
				repository: {
					...approval("requested").repository,
					web_url: "https://attacker.example/artisan-editor/protocol",
				},
			}),
		).toThrow();
	});

	it("rejects private clone evidence nested in outbound wire envelopes", async () => {
		const result = {
			correlation_id: "approval_1",
			kind: "hosted.project.clone.approval.query.result",
			message_id: "approval_result_1",
			origin: "backend",
			payload: {
				approval: { ...approval("requested"), preparation: "private" },
			},
			protocol_version: 1,
			schema_version: 1,
			sent_at: timestamp,
		};
		const event = {
			causation_id: "clone_1",
			correlation_id: "clone_1",
			journal_sequence: 1,
			kind: "event",
			message_id: "approval_requested",
			origin: "backend",
			payload: {
				approval: {
					...approval("requested"),
					repository: {
						...approval("requested").repository,
						clone_url: repository().clone_url,
					},
				},
				type: "hosted.project.clone.approval.updated",
			},
			protocol_version: 1,
			schema_version: 1,
			sequence: 1,
			sent_at: timestamp,
			stream_id: "thread_1",
			thread_id: "thread_1",
		};
		const foreign_result = {
			...result,
			message_id: "approval_result_foreign",
			payload: {
				approval: {
					...approval("requested"),
					repository: {
						...approval("requested").repository,
						web_url: "https://attacker.example/artisan-editor/protocol",
					},
				},
			},
		};
		const foreign_event = {
			...event,
			message_id: "approval_requested_foreign",
			payload: {
				approval: {
					...approval("requested"),
					repository: {
						...approval("requested").repository,
						web_url: "https://attacker.example/artisan-editor/protocol",
					},
				},
				type: "hosted.project.clone.approval.updated",
			},
		};

		await expect(Effect.runPromise(DecodeOutboundControlEnvelope(result))).rejects.toThrow();
		await expect(Effect.runPromise(DecodeOutboundControlEnvelope(event))).rejects.toThrow();
		await expect(
			Effect.runPromise(DecodeOutboundControlEnvelope(foreign_result)),
		).rejects.toThrow();
		await expect(
			Effect.runPromise(DecodeOutboundControlEnvelope(foreign_event)),
		).rejects.toThrow();
	});

	it.each([
		{ ...approval("approved"), decision: "denied" },
		{ ...approval("denied"), decision: "approved" },
		{ ...approval("requested"), decision: "approved" },
		{ ...approval("reused"), decided_at: timestamp },
		{ ...approval("applied"), attachment: "unattached" },
		{ ...approval("rejected"), reason: "interrupted" },
		{ ...approval("outcome_unknown"), reason: "provider_unavailable" },
		{ ...approval("attachment_conflict"), project: undefined },
	])("rejects invalid lifecycle and decision combinations", (invalid) => {
		expect(() => decode_approval(invalid)).toThrow();
	});
});
