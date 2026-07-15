import { Effect, Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeInboundControlEnvelope,
	DecodeOutboundControlEnvelope,
	HostedGitCheckFailureDetail,
	HostedGitMutationRequest,
	HostedGitMutationResult,
	HostedGitMutationSummary,
	summarize_hosted_git_mutation,
} from "@artisan/protocol";

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
	it("accepts exact canonical writes while keeping a reply body out of the safe summary", async () => {
		const target = {
			expected_head_commit: head,
			pull_request_number: 42,
			pull_request_origin: {
				native_id: "PR_42",
				provider_id: "github",
				resource_kind: "pull_request",
			},
			repository: {
				host: "github.com",
				name: "editor",
				owner: "artisan",
				provider_id: "github",
			},
			selected_branch: "feature/hosted-state",
			snapshot_version: 1,
			workspace_id: "workspace_1",
		};
		const reply = {
			...target,
			body: "private reply text",
			operation: "reply_review_thread" as const,
			thread_origin: {
				native_id: "thread_2",
				provider_id: "github",
				resource_kind: "review_thread" as const,
			},
		};
		const requests = [
			reply,
			{
				...target,
				operation: "resolve_review_thread" as const,
				thread_origin: {
					native_id: "thread_2",
					provider_id: "github",
					resource_kind: "review_thread" as const,
				},
			},
			{
				...target,
				operation: "request_reviewers" as const,
				reviewers: [
					{ _tag: "user" as const, login: "alice" },
					{
						_tag: "team" as const,
						organization: "artisan",
						slug: "maintainers",
					},
				],
			},
			{
				...target,
				mode: "failed_only" as const,
				operation: "rerun_workflow" as const,
				workflow_origin: {
					native_id: "workflow_1",
					provider_id: "github",
					resource_kind: "workflow_run" as const,
				},
			},
			{
				...target,
				operation: "cancel_workflow" as const,
				workflow_origin: {
					native_id: "workflow_1",
					provider_id: "github",
					resource_kind: "workflow_run" as const,
				},
			},
			{ ...target, method: "rebase" as const, operation: "merge_pull_request" as const },
		];

		for (const request of requests) {
			const decoded = await Effect.runPromise(
				Schema.decodeUnknownEffect(HostedGitMutationRequest, {
					onExcessProperty: "error",
				})(request),
			);
			const summary = summarize_hosted_git_mutation(decoded);

			await expect(
				Effect.runPromise(Schema.decodeUnknownEffect(HostedGitMutationSummary)(summary)),
			).resolves.toEqual(summary);
			expect(summary).toMatchObject({
				expected_head_commit: target.expected_head_commit,
				operation: request.operation,
				pull_request_origin: target.pull_request_origin,
				repository: target.repository,
				snapshot_version: target.snapshot_version,
				workspace_id: target.workspace_id,
			});
		}

		const reply_summary = summarize_hosted_git_mutation(
			await Effect.runPromise(Schema.decodeUnknownEffect(HostedGitMutationRequest)(reply)),
		);

		expect(reply_summary).toEqual({
			...target,
			operation: "reply_review_thread",
			thread_origin: reply.thread_origin,
		});
		expect(JSON.stringify(reply_summary)).not.toContain(reply.body);
		await expect(
			Effect.runPromise(
				Schema.decodeUnknownEffect(HostedGitMutationRequest)({
					...reply,
					thread_origin: { ...reply.thread_origin, provider_id: "gitlab" },
				}),
			),
		).rejects.toBeDefined();
		await expect(
			Effect.runPromise(
				Schema.decodeUnknownEffect(HostedGitMutationRequest, {
					onExcessProperty: "error",
				})({ ...reply, client_mutation_id: "renderer_owned_id" }),
			),
		).rejects.toBeDefined();
		await expect(
			Effect.runPromise(
				Schema.decodeUnknownEffect(HostedGitMutationRequest)({
					...target,
					operation: "request_reviewers",
					reviewers: [
						{ _tag: "user", login: "Alice" },
						{ _tag: "user", login: "alice" },
					],
				}),
			),
		).rejects.toBeDefined();

		await expect(
			Effect.runPromise(
				Schema.decodeUnknownEffect(HostedGitMutationSummary, { onExcessProperty: "error" })(
					{ ...reply_summary, body: reply.body },
				),
			),
		).rejects.toBeDefined();
	});

	it("requires the exact native origin kind for every successful hosted write", async () => {
		const cases = [
			{ operation: "reply_review_thread", resource_kind: "review_comment" },
			{ operation: "resolve_review_thread", resource_kind: "review_thread" },
			{ operation: "request_reviewers", resource_kind: "pull_request" },
			{ operation: "rerun_workflow", resource_kind: "workflow_run" },
			{ operation: "cancel_workflow", resource_kind: "workflow_run" },
			{ operation: "merge_pull_request", resource_kind: "pull_request" },
		] as const;

		for (const test_case of cases) {
			const result = {
				operation: test_case.operation,
				origin: {
					native_id: `${test_case.operation}_native`,
					provider_id: "github",
					resource_kind: test_case.resource_kind,
				},
				status: "applied",
			};

			await expect(
				Effect.runPromise(Schema.decodeUnknownEffect(HostedGitMutationResult)(result)),
			).resolves.toEqual(result);
			await expect(
				Effect.runPromise(
					Schema.decodeUnknownEffect(HostedGitMutationResult)({
						...result,
						origin: { ...result.origin, resource_kind: "check_run" },
					}),
				),
			).rejects.toBeDefined();
		}

		await expect(
			Effect.runPromise(
				Schema.decodeUnknownEffect(HostedGitMutationResult)({
					operation: "merge_pull_request",
					status: "applied",
				}),
			),
		).rejects.toBeDefined();
	});
	it("accepts bounded attributed failure detail and rejects executable control output", async () => {
		const detail = {
			attempt: 2,
			check_origin: {
				native_id: "CR_1",
				provider_id: "github",
				resource_kind: "check_run",
			},
			head_commit: head,
			log: {
				_tag: "available",
				observed_bytes: 80_000,
				truncated: true,
				untrusted_excerpt: "FAIL src/main.ts\nExpected a string",
			},
			name: "test",
			output: {
				summary: {
					_tag: "available",
					truncated: false,
					untrusted_text: "One job failed",
				},
				text: { _tag: "unavailable" },
				title: "Tests failed",
			},
			workflow_origin: {
				native_id: "WR_1",
				provider_id: "github",
				resource_kind: "workflow_run",
			},
		};

		await expect(
			Effect.runPromise(Schema.decodeUnknownEffect(HostedGitCheckFailureDetail)(detail)),
		).resolves.toEqual(detail);

		await expect(
			Effect.runPromise(
				Schema.decodeUnknownEffect(HostedGitCheckFailureDetail)({
					...detail,
					log: {
						...detail.log,
						untrusted_excerpt: "safe\u001b[2Jforged",
					},
				}),
			),
		).rejects.toBeDefined();
	});

	it("roundtrips query, refresh, failure detail, result, and durable snapshot event envelopes", async () => {
		const inbound = [
			frontend_envelope("hosted.git.snapshot.query", { workspace_id: "workspace_1" }),
			frontend_envelope("hosted.git.check_failure_detail.query", {
				check_origin: {
					native_id: "CR_1",
					provider_id: "github",
					resource_kind: "check_run",
				},
				expected_head_commit: head,
				snapshot_version: 1,
				workspace_id: "workspace_1",
			}),
			{
				...frontend_envelope("hosted.git.snapshot.refresh", {
					workspace_id: "workspace_1",
				}),
				thread_id: "thread_1",
			},
		];
		const outbound = [
			{
				correlation_id: "detail_query_1",
				kind: "hosted.git.check_failure_detail.query.result",
				message_id: "detail_result_1",
				origin: "backend",
				payload: {
					detail: {
						check_origin: {
							native_id: "CR_1",
							provider_id: "github",
							resource_kind: "check_run",
						},
						head_commit: head,
						log: { _tag: "unavailable", reason: "not_available" },
						name: "test",
						output: {
							summary: {
								_tag: "available",
								truncated: false,
								untrusted_text: "Failed",
							},
							text: { _tag: "unavailable" },
						},
					},
					journal_sequence: 4,
					observed_at: timestamp,
					snapshot_version: 1,
					workspace_id: "workspace_1",
				},
				protocol_version: 1,
				schema_version: 1,
				sent_at: timestamp,
			},
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
