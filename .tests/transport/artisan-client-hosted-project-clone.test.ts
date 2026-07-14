import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { ArtisanClientError, type ArtisanHostedProjectCloneInput } from "@artisan/transport";

import { make_transport_test_harness } from "./message-channel-harness";

function make_clone_request(): ArtisanHostedProjectCloneInput {
	return {
		command_id: "hosted_clone_fixture",
		destination_path: "C:\\Projects\\artisan-editor",
		repository: {
			archived: false,
			clone_url: "https://github.com/artisan/artisan-editor.git",
			default_branch: { _tag: "known", name: "main" },
			identity: {
				host: "github.com",
				name: "artisan-editor",
				owner: "artisan",
				provider_id: "github",
			},
			origin: {
				native_id: "R_artisan_editor",
				provider_id: "github",
				resource_kind: "repository",
			},
			viewer_permission: "admin",
			visibility: "private",
			web_url: "https://github.com/artisan/artisan-editor",
		},
		selection: {
			account_login: "sander",
			host: "github.com",
			provider_id: "github",
		},
		thread_id: "thread_git_fixture",
	};
}

describe("ArtisanClient hosted project clone routes", () => {
	it("correlates source-free approval queries and preserves clone command intent", async () => {
		const harness = await make_transport_test_harness();

		try {
			const approval = await Effect.runPromise(
				harness.client.GetHostedProjectCloneApproval({
					approval_id: "hosted_clone_approval_fixture",
					thread_id: "thread_git_fixture",
				}),
			);
			const request_receipt = await Effect.runPromise(
				harness.client.RequestHostedProjectClone(make_clone_request()),
			);
			const response_receipt = await Effect.runPromise(
				harness.client.RespondHostedProjectCloneApproval({
					approval_id: "hosted_clone_approval_fixture",
					approved: true,
					command_id: "hosted_clone_response_fixture",
					thread_id: "thread_git_fixture",
				}),
			);
			const snapshot = harness.protocol_snapshot();

			expect(approval.approval).toMatchObject({
				approval_id: "hosted_clone_approval_fixture",
				repository: {
					host: "github.com",
					name: "artisan-editor",
					owner: "artisan",
				},
				state: "requested",
			});
			expect(approval.approval.repository).not.toHaveProperty("clone_url");
			expect(approval.approval.repository).not.toHaveProperty("origin");
			expect([request_receipt, response_receipt]).toMatchObject([
				{ command_id: "hosted_clone_fixture", status: "accepted" },
				{ command_id: "hosted_clone_response_fixture", status: "accepted" },
			]);
			expect(snapshot.hosted_project_clone_approval_query_attempts).toMatchObject([
				{
					kind: "hosted.project.clone.approval.query",
					payload: {
						approval_id: "hosted_clone_approval_fixture",
						thread_id: "thread_git_fixture",
					},
				},
			]);
			expect(snapshot.hosted_project_clone_request_attempts).toMatchObject([
				{
					kind: "hosted.project.clone.request",
					message_id: "hosted_clone_fixture",
					payload: {
						destination_path: "C:\\Projects\\artisan-editor",
						repository: {
							clone_url: "https://github.com/artisan/artisan-editor.git",
							origin: { native_id: "R_artisan_editor" },
						},
						selection: { account_login: "sander", provider_id: "github" },
					},
					thread_id: "thread_git_fixture",
				},
			]);
			expect(snapshot.hosted_project_clone_approval_response_attempts).toMatchObject([
				{
					kind: "hosted.project.clone.approval.respond",
					message_id: "hosted_clone_response_fixture",
					payload: { approval_id: "hosted_clone_approval_fixture", approved: true },
					thread_id: "thread_git_fixture",
				},
			]);
		} finally {
			await harness.dispose();
		}
	});

	it("rejects invalid clone URLs before sending provider intent", async () => {
		const harness = await make_transport_test_harness();

		try {
			const request = make_clone_request();
			const error = await Effect.runPromise(
				harness.client.RequestHostedProjectClone({
					...request,
					repository: {
						...request.repository,
						clone_url: "https://sander:secret@github.com/artisan/artisan-editor.git",
					},
				}),
			).catch((cause: unknown) => cause);

			expect(error).toBeInstanceOf(ArtisanClientError);
			expect(error).toMatchObject({ code: "malformed", retryable: false });
			expect(harness.protocol_snapshot().hosted_project_clone_request_attempts).toEqual([]);
		} finally {
			await harness.dispose();
		}
	});
});
