import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { ArtisanClientError } from "@artisan/transport";

import { make_transport_test_harness, wait_for } from "./message-channel-harness";

describe("ArtisanClient Git session routes", () => {
	it("correlates both Git queries and resolves all mutation receipts", async () => {
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
			expect(receipts.map(({ command_id, status }) => ({ command_id, status }))).toEqual([
				{ command_id: "git_refresh_fixture", status: "accepted" },
				{ command_id: "git_checkout_fixture", status: "accepted" },
				{ command_id: "git_approval_response_fixture", status: "accepted" },
			]);
			expect(harness.protocol_snapshot().received_kinds).toEqual(
				expect.arrayContaining([
					"workspace.git.session.query",
					"workspace.git.checkout.approval.query",
					"workspace.git.session.refresh",
					"workspace.git.checkout.request",
					"workspace.git.checkout.approval.respond",
				]),
			);
		} finally {
			await harness.dispose();
		}
	});

	it("preserves rejected receipt protocol details and retries with the same command id", async () => {
		const harness = await make_transport_test_harness({
			client: { reconnect_delay_ms: 5 },
			drop_first_command_receipt: true,
		});

		try {
			const rejected = Effect.runPromise(
				harness.client.RequestWorkspaceGitCheckout({
					command_id: "git_rejected_checkout",
					expected_session_version: 2,
					target_branch: "reject",
					thread_id: "thread_git_fixture",
					workspace_id: "workspace_git_fixture",
				}),
			).catch((error: unknown) => error);
			const error = await rejected;

			expect(error).toBeInstanceOf(ArtisanClientError);
			expect(error).toMatchObject({
				code: "protocol",
				protocol_code: "workspace.git.checkout.blocked",
				retryable: true,
			});
			await wait_for(
				() =>
					harness
						.protocol_snapshot()
						.workspace_git_mutation_attempts.filter(
							({ message_id }) => message_id === "git_rejected_checkout",
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
			expect(
				harness
					.protocol_snapshot()
					.workspace_git_mutation_attempts.filter(
						({ message_id }) => message_id === "git_rejected_checkout",
					)
					.map(({ message_id }) => message_id),
			).toEqual(["git_rejected_checkout", "git_rejected_checkout"]);
		} finally {
			await harness.dispose();
		}
	});
});
