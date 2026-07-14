import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { make_transport_test_harness, wait_for } from "./message-channel-harness";

describe("ArtisanClient Git fetch routes", () => {
	it("correlates the global query and scopes policy and manual-fetch receipts", async () => {
		const harness = await make_transport_test_harness();

		try {
			const query = await Effect.runPromise(harness.client.GetWorkspaceGitFetch);
			const [policy_receipt, manual_receipt] = await Effect.runPromise(
				Effect.all([
					harness.client.UpdateWorkspaceGitFetchPolicy({
						command_id: "git_fetch_policy_fixture",
						enabled: true,
					}),
					harness.client.RequestWorkspaceGitFetch({
						command_id: "git_fetch_manual_fixture",
						thread_id: "thread_git_fixture",
						workspace_id: "workspace_git_fixture",
					}),
				]),
			);

			expect(query).toEqual({
				enabled: false,
				workspaces: [{ workspace_id: "workspace_git_fixture" }],
			});
			expect(policy_receipt).toMatchObject({
				command_id: "git_fetch_policy_fixture",
				status: "accepted",
			});
			expect(manual_receipt).toMatchObject({
				command_id: "git_fetch_manual_fixture",
				status: "accepted",
			});

			const snapshot = harness.protocol_snapshot();

			expect(snapshot.received_kinds).toEqual(
				expect.arrayContaining([
					"workspace.git.fetch.query",
					"workspace.git.fetch.policy.update",
					"workspace.git.fetch.request",
				]),
			);
			expect(snapshot.workspace_git_fetch_policy_update_attempts).toMatchObject([
				{
					message_id: "git_fetch_policy_fixture",
					payload: { enabled: true },
				},
			]);
			expect(snapshot.workspace_git_fetch_request_attempts).toMatchObject([
				{
					message_id: "git_fetch_manual_fixture",
					payload: { workspace_id: "workspace_git_fixture" },
					thread_id: "thread_git_fixture",
				},
			]);
		} finally {
			await harness.dispose();
		}
	});

	it("retries exact fetch query and policy envelopes after dropped results and receipts", async () => {
		const harness = await make_transport_test_harness({
			client: { reconnect_delay_ms: 10 },
			drop_first_command_receipt: true,
			protocol: { drop_first_workspace_git_fetch_result: true },
		});

		try {
			const query = await Effect.runPromise(harness.client.GetWorkspaceGitFetch);

			expect(query.enabled).toBe(false);
			await wait_for(
				() => harness.protocol_snapshot().workspace_git_fetch_query_attempts.length === 2,
			);
			const query_attempts = harness.protocol_snapshot().workspace_git_fetch_query_attempts;

			expect(query_attempts[1]).toEqual(query_attempts[0]);

			const receipt = await Effect.runPromise(
				harness.client.UpdateWorkspaceGitFetchPolicy({
					command_id: "git_fetch_policy_retry",
					enabled: true,
				}),
			);

			expect(receipt).toMatchObject({
				command_id: "git_fetch_policy_retry",
				status: "duplicate",
			});
			await wait_for(
				() =>
					harness.protocol_snapshot().workspace_git_fetch_policy_update_attempts
						.length === 2,
			);
			const policy_attempts =
				harness.protocol_snapshot().workspace_git_fetch_policy_update_attempts;

			expect(policy_attempts[1]).toEqual(policy_attempts[0]);
		} finally {
			await harness.dispose();
		}
	});

	it("retries the exact manual fetch envelope after a dropped receipt", async () => {
		const harness = await make_transport_test_harness({
			client: { reconnect_delay_ms: 10 },
			drop_first_command_receipt: true,
		});

		try {
			const receipt = await Effect.runPromise(
				harness.client.RequestWorkspaceGitFetch({
					command_id: "git_fetch_manual_retry",
					thread_id: "thread_git_fixture",
					workspace_id: "workspace_git_fixture",
				}),
			);

			expect(receipt).toMatchObject({
				command_id: "git_fetch_manual_retry",
				status: "duplicate",
			});
			await wait_for(
				() => harness.protocol_snapshot().workspace_git_fetch_request_attempts.length === 2,
			);
			const attempts = harness.protocol_snapshot().workspace_git_fetch_request_attempts;

			expect(attempts[1]).toEqual(attempts[0]);
		} finally {
			await harness.dispose();
		}
	});
});
