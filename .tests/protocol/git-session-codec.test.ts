import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { DecodeInboundControlEnvelope, DecodeOutboundControlEnvelope } from "@artisan/protocol";

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

function session() {
	return {
		blockers: [],
		branch: "main",
		changed_files: [
			{
				conflicted: false,
				path: "src/main.ts",
				staged: true,
				status: "modified",
				untracked: false,
				unstaged: false,
			},
		],
		diff_stats: { additions: 1, deletions: 0, files: 1 },
		has_diff: true,
		head,
		journal_sequence: 3,
		observed_at: timestamp,
		state: "ready",
		version: 2,
		worktrees: [
			{
				bare: false,
				branch: "main",
				detached: false,
				head,
				locked: false,
				location: "selected",
				prunable: false,
			},
		],
		workspace_id: "workspace_1",
	};
}

function approval(state: string) {
	const base = {
		approval_id: "approval_1",
		created_at: timestamp,
		expected_session_version: 2,
		source_branch: "main",
		source_command_id: "checkout_1",
		source_head: head,
		target_branch: "release",
		thread_id: "thread_1",
		updated_at: timestamp,
		workspace_id: "workspace_1",
	};

	return state === "requested"
		? { ...base, state }
		: {
				...base,
				decided_at: timestamp,
				decision: state === "denied" ? "denied" : "approved",
				decision_message_id: "decision_1",
				state,
			};
}

describe("Git session protocol codec", () => {
	it("roundtrips session, checkout, and approval envelopes", async () => {
		const inbound = [
			frontend_envelope("workspace.git.session.query", { workspace_id: "workspace_1" }),
			{
				...frontend_envelope("workspace.git.session.refresh", {
					workspace_id: "workspace_1",
				}),
				thread_id: "thread_1",
			},
			{
				...frontend_envelope("workspace.git.checkout.request", {
					expected_session_version: 2,
					target_branch: "release",
					workspace_id: "workspace_1",
				}),
				thread_id: "thread_1",
			},
			frontend_envelope("workspace.git.checkout.approval.query", {
				approval_id: "approval_1",
				thread_id: "thread_1",
			}),
			{
				...frontend_envelope("workspace.git.checkout.approval.respond", {
					approval_id: "approval_1",
					approved: true,
				}),
				thread_id: "thread_1",
			},
		];
		const outbound = [
			{
				correlation_id: "session_1",
				kind: "workspace.git.session.query.result",
				message_id: "session_result_1",
				origin: "backend",
				payload: { journal_sequence: 3, session: session() },
				protocol_version: 1,
				schema_version: 1,
				sent_at: timestamp,
			},
			{
				correlation_id: "session_absent_1",
				kind: "workspace.git.session.query.result",
				message_id: "session_absent_result_1",
				origin: "backend",
				payload: { journal_sequence: 3 },
				protocol_version: 1,
				schema_version: 1,
				sent_at: timestamp,
			},
			{
				correlation_id: "approval_1",
				kind: "workspace.git.checkout.approval.query.result",
				message_id: "approval_result_1",
				origin: "backend",
				payload: { approval: approval("requested") },
				protocol_version: 1,
				schema_version: 1,
				sent_at: timestamp,
			},
			{
				causation_id: "refresh_1",
				correlation_id: "refresh_1",
				journal_sequence: 4,
				kind: "event",
				message_id: "session_updated_1",
				origin: "backend",
				payload: { session: session(), type: "workspace.git.session.updated" },
				protocol_version: 1,
				schema_version: 1,
				sequence: 1,
				sent_at: timestamp,
				stream_id: "thread_1",
				thread_id: "thread_1",
			},
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
		}
	});

	it.each(["requested", "approved", "executing", "applied", "rejected", "unknown", "denied"])(
		"roundtrips the %s checkout approval state",
		async (state) => {
			const envelope = {
				causation_id: "checkout_1",
				correlation_id: "checkout_1",
				journal_sequence: 4,
				kind: "event",
				message_id: `approval_${state}`,
				origin: "backend",
				payload: {
					approval: approval(state),
					type: "workspace.git.checkout.approval.updated",
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

	it.each([
		{ branch: "", path: "src/main.ts", head },
		{ branch: "main\u0000", path: "src/main.ts", head },
		{ branch: "main", path: "C:/repo/main.ts", head },
		{ branch: "main", path: "src/../main.ts", head },
		{ branch: "main", path: "src/main.ts", head: "A".repeat(40) },
		{ branch: "main", path: "src/main.ts", head: "a".repeat(39) },
	])("rejects malformed Git branch, path, or object id", async ({ branch, path, head }) => {
		const invalid_session = session();

		invalid_session.branch = branch;
		invalid_session.head = head;
		invalid_session.changed_files[0]!.path = path;

		await expect(
			Effect.runPromise(
				DecodeOutboundControlEnvelope({
					correlation_id: "session_1",
					kind: "workspace.git.session.query.result",
					message_id: "session_result_1",
					origin: "backend",
					payload: { journal_sequence: 3, session: invalid_session },
					protocol_version: 1,
					schema_version: 1,
					sent_at: timestamp,
				}),
			),
		).rejects.toBeDefined();
	});
});
