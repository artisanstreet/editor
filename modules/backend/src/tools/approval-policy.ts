import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	ArtisanToolDescriptor,
	ArtisanToolPermissionPolicy,
	type ArtisanToolDescriptor as ArtisanToolDescriptorValue,
	type ArtisanToolPermissionDecision as ArtisanToolPermissionDecisionValue,
	type ArtisanToolPermissionPolicy as ArtisanToolPermissionPolicyValue,
} from "@artisan/protocol";

/** Reports malformed policy or descriptor input at the control-plane boundary. */
export class ArtisanToolApprovalPolicyFailure extends Data.TaggedError(
	"ArtisanToolApprovalPolicyFailure",
)<{ readonly cause: unknown }> {}

const permission_key = {
	engine_observation: "allow_engine_observation",
	git_index_write: "allow_git_index_write",
	git_read: undefined,
	none: undefined,
	preview_control: "allow_preview_control",
	process_control: "allow_process_control",
	user_interaction: undefined,
	workspace_read: "allow_workspace_read",
	workspace_write: "allow_workspace_write",
} as const;

/** Evaluates descriptor-declared permissions against the selected session policy. */
export class ArtisanToolApprovalPolicy extends Context.Service<
	ArtisanToolApprovalPolicy,
	{
		readonly Decide: (
			descriptor: ArtisanToolDescriptorValue,
			policy: ArtisanToolPermissionPolicyValue,
		) => Effect.Effect<ArtisanToolPermissionDecisionValue, ArtisanToolApprovalPolicyFailure>;
	}
>()("Artisan/ArtisanToolApprovalPolicy") {}

export const ArtisanToolApprovalPolicyLive = Layer.succeed(ArtisanToolApprovalPolicy, {
	Decide: (descriptor, policy) =>
		Effect.gen(function* () {
			const decoded_descriptor = yield* Schema.decodeUnknownEffect(ArtisanToolDescriptor, {
				onExcessProperty: "error",
			})(descriptor).pipe(
				Effect.mapError((cause) => new ArtisanToolApprovalPolicyFailure({ cause })),
			);
			const decoded_policy = yield* Schema.decodeUnknownEffect(ArtisanToolPermissionPolicy, {
				onExcessProperty: "error",
			})(policy).pipe(
				Effect.mapError((cause) => new ArtisanToolApprovalPolicyFailure({ cause })),
			);
			const denied_requirement = decoded_descriptor.permission_requirements.find(
				(requirement) => {
					const key = permission_key[requirement];

					return key !== undefined && !decoded_policy[key];
				},
			);

			if (denied_requirement !== undefined) {
				return {
					decision: "denied",
					policy: decoded_policy,
					reason: `Session policy denies ${denied_requirement}`,
					requirements: decoded_descriptor.permission_requirements,
					tool_id: decoded_descriptor.id,
				};
			}

			const has_sensitive_requirement = decoded_descriptor.permission_requirements.some(
				(requirement) => requirement !== "none",
			);
			const decision =
				has_sensitive_requirement &&
				(decoded_policy.approval === "always" ||
					(decoded_policy.approval === "on_request" &&
						decoded_descriptor.approval_behavior === "on_request"))
					? "approval_required"
					: "allowed";

			return {
				decision,
				policy: decoded_policy,
				requirements: decoded_descriptor.permission_requirements,
				tool_id: decoded_descriptor.id,
			};
		}),
});
