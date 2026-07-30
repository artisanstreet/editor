import { Effect } from "effect";

import type { InboundControlEnvelope, MarketplaceScope } from "@artisan/protocol";

import { CapabilityMirrorService } from "../../../marketplace/capabilities/provider-mirrors";
import { CapabilityRepository } from "../../../marketplace/capabilities/repository";
import {
	CapabilityOAuthLifecycle,
	CapabilityService,
} from "../../../marketplace/capabilities/service";
import type { ReadyState } from "../../connection-state";
import { MakeMarketplaceResponse } from "./marketplace-response";

export type CapabilityMutationEnvelope = Extract<
	InboundControlEnvelope,
	{ readonly kind: `marketplace.capability.${string}` }
>;

const ScopeMatches = (left: MarketplaceScope, right: MarketplaceScope) =>
	left.kind === right.kind &&
	(left.kind === "global" ||
		(left.kind === "workspace" &&
			right.kind === "workspace" &&
			left.workspace_id === right.workspace_id) ||
		(left.kind === "project" &&
			right.kind === "project" &&
			left.project_id === right.project_id));

export const MakeCapabilityMutationHandler = Effect.gen(function* () {
	const capabilities = yield* CapabilityService;
	const repository = yield* CapabilityRepository;
	const oauth = yield* CapabilityOAuthLifecycle;
	const mirrors = yield* CapabilityMirrorService;
	const response = yield* MakeMarketplaceResponse;

	const RequireScope = <Success, Error>(
		capability_id: string,
		scope: MarketplaceScope,
		program: Effect.Effect<Success, Error>,
	) =>
		repository.ReadDetail(capability_id).pipe(
			Effect.filterOrFail(
				(detail) => ScopeMatches(detail.scope, scope),
				() => "Capability is outside the requested Marketplace scope",
			),
			Effect.andThen(program),
		);

	const Action = (
		envelope: CapabilityMutationEnvelope,
		program: Effect.Effect<unknown, unknown>,
	) => response.Action(envelope, program);

	const Handle = (envelope: CapabilityMutationEnvelope, current: ReadyState) => {
		switch (envelope.kind) {
			case "marketplace.capability.connect.request":
				return Action(
					envelope,
					capabilities
						.Preview({
							auth: envelope.payload.auth,
							scope: envelope.payload.scope,
							source: envelope.payload.source,
							transport: envelope.payload.transport,
						})
						.pipe(
							Effect.filterOrFail(
								(preview) =>
									preview.preview_fingerprint ===
									envelope.payload.preview_fingerprint,
								() => "Capability preview changed; approval must be renewed",
							),
							Effect.flatMap((preview) =>
								capabilities.RequestConnect({
									approval_id: envelope.payload.approval_id,
									detail: {
										auth: envelope.payload.auth,
										compatibility: [...preview.compatibility],
										display_name: preview.candidate_name,
										enabled: true,
										health: { status: "unknown" },
										id: preview.candidate_id,
										lifecycle: "awaiting_approval",
										permissions: [...preview.permissions],
										policy: [],
										resources: [],
										scope: preview.scope,
										source: preview.source,
										status: "awaiting_approval",
										sync: [],
										tools: [...preview.tools],
										transport: preview.transport,
										...(preview.transport_policy === undefined
											? {}
											: { transport_policy: preview.transport_policy }),
										trust: preview.trust,
									},
									operation_id: envelope.message_id,
									preview_fingerprint: preview.preview_fingerprint,
									request_fingerprint: envelope.message_id,
								}),
							),
						),
				);
			case "marketplace.capability.connect.decision":
				return Action(
					envelope,
					capabilities.DecideConnect({
						approval_fingerprint: envelope.payload.preview_fingerprint,
						approval_id: envelope.payload.approval_id,
						approved: envelope.payload.approved,
					}),
				);
			case "marketplace.capability.start":
			case "marketplace.capability.reconnect":
			case "marketplace.capability.restart":
				return Action(
					envelope,
					RequireScope(
						envelope.payload.capability_id,
						envelope.payload.scope,
						capabilities.SessionAction({
							action:
								envelope.kind === "marketplace.capability.start"
									? "start"
									: envelope.kind === "marketplace.capability.restart"
										? "restart"
										: "reconnect",
							capability_id: envelope.payload.capability_id,
							operation_id: envelope.message_id,
						}),
					),
				);
			case "marketplace.capability.invoke":
				return RequireScope(
					envelope.payload.capability_id,
					envelope.payload.scope,
					capabilities.Invoke({
						...envelope.payload,
						operation_id: envelope.message_id,
					}),
				).pipe(
					Effect.flatMap((payload) =>
						response.Result(
							envelope,
							current,
							"marketplace.capability.invoke.result",
							Effect.succeed(payload),
						),
					),
					Effect.catch(() =>
						response.Reject(
							envelope,
							current,
							"The Marketplace action was rejected before completion.",
						),
					),
				);
			case "marketplace.capability.invoke.request":
			case "marketplace.capability.invoke.decision":
				return RequireScope(
					envelope.payload.capability_id,
					envelope.payload.scope,
					envelope.kind === "marketplace.capability.invoke.request"
						? capabilities.RequestInvocation({
								...envelope.payload,
								operation_id: envelope.message_id,
							})
						: capabilities.DecideInvocation(envelope.payload),
				).pipe(
					Effect.flatMap((payload) =>
						response.Result(
							envelope,
							current,
							"marketplace.capability.invoke.result",
							Effect.succeed(payload),
						),
					),
					Effect.catch(() =>
						response.Reject(
							envelope,
							current,
							"The Marketplace invocation approval was rejected.",
						),
					),
				);
			case "marketplace.capability.drift.overwrite.request":
			case "marketplace.capability.drift.overwrite.decision":
				return Action(
					envelope,
					RequireScope(
						envelope.payload.capability_id,
						envelope.payload.scope,
						envelope.kind === "marketplace.capability.drift.overwrite.request"
							? mirrors.RequestOverwrite({
									approval_fingerprint: envelope.payload.intent_fingerprint,
									approval_id: envelope.payload.approval_id,
									capability_id: envelope.payload.capability_id,
									engine_id: envelope.payload.engine_id,
									observed_revision: envelope.payload.observed_revision,
									operation_id: envelope.message_id,
									scope: envelope.payload.scope,
								})
							: mirrors.DecideOverwrite({
									approval_fingerprint: envelope.payload.intent_fingerprint,
									approval_id: envelope.payload.approval_id,
									approved: envelope.payload.approved,
									capability_id: envelope.payload.capability_id,
									engine_id: envelope.payload.engine_id,
									observed_revision: envelope.payload.observed_revision,
									scope: envelope.payload.scope,
								}),
					),
				);
			case "marketplace.capability.enable":
				return Action(
					envelope,
					RequireScope(
						envelope.payload.id,
						envelope.payload.scope,
						capabilities.Enable({
							capability_id: envelope.payload.id,
							operation_id: envelope.message_id,
						}),
					),
				);
			case "marketplace.capability.disable":
				return Action(
					envelope,
					RequireScope(
						envelope.payload.id,
						envelope.payload.scope,
						capabilities.Disable({
							capability_id: envelope.payload.id,
							operation_id: envelope.message_id,
						}),
					),
				);
			case "marketplace.capability.remove":
				return Action(
					envelope,
					RequireScope(
						envelope.payload.id,
						envelope.payload.scope,
						capabilities.Remove({
							capability_id: envelope.payload.id,
							operation_id: envelope.message_id,
						}),
					),
				);
			case "marketplace.capability.disconnect":
				return Action(
					envelope,
					RequireScope(
						envelope.payload.capability_id,
						envelope.payload.scope,
						capabilities.Disconnect({
							capability_id: envelope.payload.capability_id,
							operation_id: envelope.message_id,
						}),
					),
				);
			case "marketplace.capability.uninstall":
				return Action(
					envelope,
					RequireScope(
						envelope.payload.capability_id,
						envelope.payload.scope,
						capabilities.Uninstall({
							capability_id: envelope.payload.capability_id,
							operation_id: envelope.message_id,
						}),
					),
				);
			case "marketplace.capability.health":
				return Action(
					envelope,
					RequireScope(
						envelope.payload.capability_id,
						envelope.payload.scope,
						capabilities.Health({
							capability_id: envelope.payload.capability_id,
							operation_id: envelope.message_id,
						}),
					),
				);
			case "marketplace.capability.sync":
				return Action(
					envelope,
					RequireScope(
						envelope.payload.id,
						envelope.payload.scope,
						mirrors.Sync({
							capability_id: envelope.payload.id,
							engine_id: envelope.payload.engine_id,
							operation_id: envelope.message_id,
						}),
					),
				);
			case "marketplace.capability.drift.resolve":
				return Action(
					envelope,
					RequireScope(
						envelope.payload.capability_id,
						envelope.payload.scope,
						mirrors.ResolveDrift({
							...envelope.payload,
							operation_id: envelope.message_id,
						}),
					),
				);
			case "marketplace.capability.oauth.begin":
				return response.Result(
					envelope,
					current,
					"marketplace.capability.oauth.begin.result",
					repository.ReadDetail(envelope.payload.capability_id).pipe(
						Effect.filterOrFail(
							(detail) => ScopeMatches(detail.scope, envelope.payload.scope),
							() => "Capability is outside the requested Marketplace scope",
						),
						Effect.flatMap((detail) =>
							detail.auth.kind === "oauth"
								? oauth
										.Begin({
											authorization_url: detail.auth.authorization_url,
											capability_id: detail.id,
											operation_id: envelope.message_id,
											scopes: detail.auth.scopes,
										})
										.pipe(
											Effect.filterOrFail(
												(result) => result._tag === "started",
												() =>
													"OAuth begin result is unavailable for this retry",
											),
											Effect.map((result) => ({
												authorization_url: result.authorization_url,
												continuation_reference: result.state,
											})),
										)
								: Effect.fail("oauth unavailable"),
						),
					),
				);
			case "marketplace.capability.oauth.complete":
				return Action(
					envelope,
					RequireScope(
						envelope.payload.capability_id,
						envelope.payload.scope,
						oauth.Complete({
							capability_id: envelope.payload.capability_id,
							callback_reference: envelope.payload.callback_reference,
							operation_id: envelope.message_id,
						}),
					),
				);
			case "marketplace.capability.oauth.refresh":
				return Action(
					envelope,
					RequireScope(
						envelope.payload.capability_id,
						envelope.payload.scope,
						oauth.Refresh({
							capability_id: envelope.payload.capability_id,
							operation_id: envelope.message_id,
						}),
					),
				);
			case "marketplace.capability.oauth.revoke":
				return Action(
					envelope,
					RequireScope(
						envelope.payload.capability_id,
						envelope.payload.scope,
						oauth.Revoke({
							capability_id: envelope.payload.capability_id,
							operation_id: envelope.message_id,
						}),
					),
				);
		}
		return Effect.die(new Error(`Unsupported capability mutation: ${envelope.kind}`));
	};

	return Handle;
});
