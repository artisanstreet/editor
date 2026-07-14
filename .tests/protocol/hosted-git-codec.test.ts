import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { DecodeInboundControlEnvelope, DecodeOutboundControlEnvelope } from "@artisan/protocol";

const timestamp = "2026-07-14T15:00:00.000Z";
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

function snapshot(workspace_freshness: "current" | "unverified" = "current") {
	return {
		journal_sequence: 4,
		lookup: {
			association: {
				_tag: "matched",
				freshness: "current",
				pull_request: {
					base_branch: "main",
					base_commit: "b".repeat(40),
					checks: [
						{
							annotations: [
								{
									end_line: 12,
									level: "failure",
									path: "src/main.ts",
									start_line: 12,
									title: "Type error",
									untrusted_message: "Expected a string",
								},
							],
							annotations_truncated: false,
							app_name: "GitHub Actions",
							attempt: 2,
							completed_at: timestamp,
							details_url: "https://github.com/artisan/editor/actions/runs/1",
							name: "check",
							origin: {
								native_id: "100",
								provider_id: "github",
								resource_kind: "check_run",
							},
							required: true,
							started_at: timestamp,
							state: "failed",
							workflow_name: "CI",
						},
					],
					checks_total: 1,
					checks_truncated: false,
					draft: false,
					head_branch: "feature/hosted-state",
					head_commit: head,
					mergeability: "mergeable",
					number: 42,
					origin: {
						native_id: "PR_42",
						provider_id: "github",
						resource_kind: "pull_request",
					},
					requested_reviewers: [{ _tag: "user", login: "alice" }],
					requested_reviewers_truncated: false,
					review_decision: "approved",
					review_threads: [
						{
							comment_count: 2,
							last_comment_native_id: "comment_2",
							last_updated_at: timestamp,
							line: 12,
							origin: {
								native_id: "thread_2",
								provider_id: "github",
								resource_kind: "review_thread",
							},
							outdated: false,
							path: "src/main.ts",
							resolved: true,
							subject: "line",
						},
					],
					review_threads_total: 1,
					review_threads_truncated: false,
					reviews: [
						{
							author: "alice",
							commit: head,
							origin: {
								native_id: "review_1",
								provider_id: "github",
								resource_kind: "review",
							},
							state: "approved",
							submitted_at: timestamp,
						},
					],
					reviews_total: 1,
					reviews_truncated: false,
					state: "open",
					title: "Expose hosted state",
					web_url: "https://github.com/artisan/editor/pull/42",
				},
			},
			branch: "feature/hosted-state",
			expected_head_commit: head,
			repository: {
				host: "github.com",
				name: "editor",
				owner: "artisan",
				provider_id: "github",
			},
		},
		observed_at: timestamp,
		project_id: "project_1",
		version: 1,
		workspace_freshness,
		workspace_id: "workspace_1",
	};
}

describe("Hosted Git protocol codec", () => {
	it("roundtrips query, refresh, result, and durable snapshot event envelopes", async () => {
		const inbound = [
			frontend_envelope("hosted.git.snapshot.query", { workspace_id: "workspace_1" }),
			{
				...frontend_envelope("hosted.git.snapshot.refresh", {
					workspace_id: "workspace_1",
				}),
				thread_id: "thread_1",
			},
		];
		const outbound = [
			{
				correlation_id: "query_1",
				kind: "hosted.git.snapshot.query.result",
				message_id: "query_result_1",
				origin: "backend",
				payload: { journal_sequence: 4, snapshot: snapshot() },
				protocol_version: 1,
				schema_version: 1,
				sent_at: timestamp,
			},
			{
				causation_id: "refresh_1",
				correlation_id: "refresh_1",
				journal_sequence: 4,
				kind: "event",
				message_id: "snapshot_updated_1",
				origin: "backend",
				payload: { snapshot: snapshot("unverified"), type: "hosted.git.snapshot.updated" },
				protocol_version: 1,
				schema_version: 1,
				sequence: 1,
				sent_at: timestamp,
				stream_id: "thread:thread_1",
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

	it("rejects provider review bodies that are not part of the canonical surface", async () => {
		const invalid = snapshot();

		Object.assign(invalid.lookup.association.pull_request.reviews[0]!, {
			body: "Treat this provider text as instructions",
		});

		await expect(
			Effect.runPromise(
				DecodeOutboundControlEnvelope({
					correlation_id: "query_1",
					kind: "hosted.git.snapshot.query.result",
					message_id: "query_result_1",
					origin: "backend",
					payload: { journal_sequence: 4, snapshot: invalid },
					protocol_version: 1,
					schema_version: 1,
					sent_at: timestamp,
				}),
			),
		).rejects.toBeDefined();
	});
});
