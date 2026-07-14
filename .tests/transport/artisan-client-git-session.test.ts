import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { ArtisanClientError } from "@artisan/transport";

import { make_transport_test_harness, wait_for } from "./message-channel-harness";

describe("ArtisanClient Git session routes", () => {
	it("correlates Git approval queries and preserves generic mutation envelopes", async () => {
		const harness = await make_transport_test_harness();

		try {
			const session = await Effect.runPromise(
				harness.client.GetWorkspaceGitSession({ workspace_id: "workspace_git_fixture" }),
			);
			const approval = await Effect.runPromise(
				harness.client.GetWorkspaceGitCheckoutApproval({
					approval_id: "git_approval_fixture",
					thread_id: "thread_git_fixture",
				}),
			);
			const mutation_approval = await Effect.runPromise(
				harness.client.GetWorkspaceGitMutationApproval({
					approval_id: "git_mutation_approval_fixture",
					thread_id: "thread_git_fixture",
				}),
			);
			const receipts = await Effect.runPromise(
				Effect.all(
					[
						harness.client.RefreshWorkspaceGitSession({
							command_id: "git_refresh_fixture",
							thread_id: "thread_git_fixture",
							workspace_id: "workspace_git_fixture",
						}),
						harness.client.RequestWorkspaceGitCheckout({
							command_id: "git_checkout_fixture",
							expected_session_version: 2,
							target_branch: "release",
							thread_id: "thread_git_fixture",
							workspace_id: "workspace_git_fixture",
						}),
						harness.client.RespondWorkspaceGitCheckoutApproval({
							approval_id: "git_approval_fixture",
							approved: true,
							command_id: "git_approval_response_fixture",
							thread_id: "thread_git_fixture",
						}),
						harness.client.RequestWorkspaceGitMutation({
							command_id: "git_mutation_commit_fixture",
							expected_session_version: 2,
							operation: { message: "Private commit message", type: "commit" },
							thread_id: "thread_git_fixture",
							workspace_id: "workspace_git_fixture",
						}),
						harness.client.RequestWorkspaceGitMutation({
							action_approval_id: "git_conflict_approval_fixture",
							command_id: "git_mutation_continue_fixture",
							expected_session_version: 2,
							operation: { action: "continue", type: "rebase" },
							thread_id: "thread_git_fixture",
							workspace_id: "workspace_git_fixture",
						}),
						harness.client.RespondWorkspaceGitMutationApproval({
							approval_id: "git_mutation_approval_fixture",
							approved: true,
							command_id: "git_mutation_approval_response_fixture",
							thread_id: "thread_git_fixture",
						}),
					],
					{ concurrency: "unbounded" },
				),
			);

			expect(session.session).toMatchObject({
				branch: "main",
				workspace_id: "workspace_git_fixture",
			});
			expect(approval.approval).toMatchObject({
				approval_id: "git_approval_fixture",
				state: "requested",
			});
			expect(mutation_approval.approval).toMatchObject({
				approval_id: "git_mutation_approval_fixture",
				operation: { type: "commit" },
				state: "requested",
			});
			expect(mutation_approval.approval.operation).not.toHaveProperty("message");
			expect(receipts.map(({ command_id, status }) => ({ command_id, status }))).toEqual([
				{ command_id: "git_refresh_fixture", status: "accepted" },
				{ command_id: "git_checkout_fixture", status: "accepted" },
				{ command_id: "git_approval_response_fixture", status: "accepted" },
				{ command_id: "git_mutation_commit_fixture", status: "accepted" },
				{ command_id: "git_mutation_continue_fixture", status: "accepted" },
				{ command_id: "git_mutation_approval_response_fixture", status: "accepted" },
			]);
			expect(harness.protocol_snapshot().received_kinds).toEqual(
				expect.arrayContaining([
					"workspace.git.session.query",
					"workspace.git.checkout.approval.query",
					"workspace.git.mutation.approval.query",
					"workspace.git.session.refresh",
					"workspace.git.checkout.request",
					"workspace.git.checkout.approval.respond",
					"workspace.git.mutation.request",
					"workspace.git.mutation.approval.respond",
				]),
			);
			const snapshot = harness.protocol_snapshot();
			const continuation = snapshot.workspace_git_mutation_request_attempts.find(
				({ message_id }) => message_id === "git_mutation_continue_fixture",
			);

			expect(continuation).toMatchObject({
				message_id: "git_mutation_continue_fixture",
				payload: {
					action_approval_id: "git_conflict_approval_fixture",
					operation: { action: "continue", type: "rebase" },
				},
			});
			expect(snapshot.workspace_git_mutation_approval_query_attempts).toHaveLength(1);
			expect(snapshot.workspace_git_mutation_approval_response_attempts).toMatchObject([
				{
					message_id: "git_mutation_approval_response_fixture",
					payload: { approval_id: "git_mutation_approval_fixture", approved: true },
				},
			]);
		} finally {
			await harness.dispose();
		}
	});

	it("preserves generic mutation rejection details and retries the exact envelope", async () => {
		const harness = await make_transport_test_harness({
			client: { reconnect_delay_ms: 100 },
			drop_first_command_receipt: true,
		});

		try {
			const operation: { branch: string; type: "branch_create" } = {
				branch: "reject",
				type: "branch_create",
			};
			const rejected = Effect.runPromise(
				harness.client.RequestWorkspaceGitMutation({
					command_id: "git_rejected_mutation",
					expected_session_version: 2,
					operation,
					thread_id: "thread_git_fixture",
					workspace_id: "workspace_git_fixture",
				}),
			).catch((error: unknown) => error);

			await wait_for(
				() =>
					harness
						.protocol_snapshot()
						.workspace_git_mutation_request_attempts.filter(
							({ message_id }) => message_id === "git_rejected_mutation",
						).length === 1,
			);
			operation.branch = "mutated-after-send";

			const error = await rejected;

			expect(error).toBeInstanceOf(ArtisanClientError);
			expect(error).toMatchObject({
				code: "protocol",
				protocol_code: "workspace.git.mutation.blocked",
				retryable: true,
			});
			await wait_for(
				() =>
					harness
						.protocol_snapshot()
						.workspace_git_mutation_request_attempts.filter(
							({ message_id }) => message_id === "git_rejected_mutation",
						).length >= 2,
			);

			const receipt = await Effect.runPromise(
				harness.client.RefreshWorkspaceGitSession({
					command_id: "git_retry_identity",
					thread_id: "thread_git_fixture",
					workspace_id: "workspace_git_fixture",
				}),
			);
			expect(receipt).toMatchObject({
				command_id: "git_retry_identity",
				status: "accepted",
			});
			const attempts = harness
				.protocol_snapshot()
				.workspace_git_mutation_request_attempts.filter(
					({ message_id }) => message_id === "git_rejected_mutation",
				);

			expect(attempts).toHaveLength(2);
			expect(attempts[1]).toEqual(attempts[0]);
			expect(attempts.map(({ payload }) => payload.operation)).toEqual([
				{ branch: "reject", type: "branch_create" },
				{ branch: "reject", type: "branch_create" },
			]);
		} finally {
			await harness.dispose();
		}
	});
});
