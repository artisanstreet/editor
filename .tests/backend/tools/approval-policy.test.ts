import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
	ArtisanToolApprovalPolicy,
	ArtisanToolApprovalPolicyLive,
} from "../../../modules/backend/src/tools/approval-policy";
import type { ArtisanToolDescriptor, ArtisanToolPermissionPolicy } from "@artisan/protocol";

const Decide = (descriptor: ArtisanToolDescriptor, policy: ArtisanToolPermissionPolicy) =>
	Effect.gen(function* () {
		const service = yield* ArtisanToolApprovalPolicy;

		return yield* service.Decide(descriptor, policy);
	}).pipe(Effect.provide(ArtisanToolApprovalPolicyLive));

const policy = {
	allow_engine_observation: true,
	allow_git_index_write: true,
	allow_preview_control: true,
	allow_process_control: true,
	allow_workspace_read: true,
	allow_workspace_write: true,
};

const descriptor = {
	approval_behavior: "on_request" as const,
	description: "Stages selected paths in the Git index.",
	id: "git.index.stage" as const,
	kind: "git" as const,
	permission_requirements: ["git_index_write" as const],
	schema_version: 1 as const,
	title: "Stage paths",
};

describe("ArtisanToolApprovalPolicy", () => {
	it("denies a descriptor when its capability is disabled", async () => {
		const result = await Effect.runPromise(
			Decide(descriptor, {
				...policy,
				allow_git_index_write: false,
				approval: "never",
			}),
		);

		expect(result).toMatchObject({ decision: "denied", tool_id: "git.index.stage" });
	});

	it("requires approval only for on-request descriptors under on-request policy", async () => {
		const requested = await Effect.runPromise(
			Decide(
				{ ...descriptor, approval_behavior: "on_request" },
				{ ...policy, approval: "on_request" },
			),
		);
		const automatic = await Effect.runPromise(
			Decide(
				{ ...descriptor, approval_behavior: "never" },
				{ ...policy, approval: "on_request" },
			),
		);

		expect(requested.decision).toBe("approval_required");
		expect(automatic.decision).toBe("allowed");
	});

	it("always requires approval for a sensitive allowed tool", async () => {
		const result = await Effect.runPromise(
			Decide(
				{ ...descriptor, approval_behavior: "never" },
				{ ...policy, approval: "always" },
			),
		);

		expect(result.decision).toBe("approval_required");
	});
});
