import { createHash } from "node:crypto";

import { Effect } from "effect";

import type { InboundControlEnvelope, MarketplaceScope } from "@artisan/protocol";

import { RoutineRepository } from "../../../marketplace/routines/repository";
import { RoutineService } from "../../../marketplace/routines/service";
import type { ReadyState } from "../../connection-state";
import { MakeMarketplaceResponse } from "./marketplace-response";

export type RoutineMutationEnvelope = Extract<
	InboundControlEnvelope,
	{
		readonly kind: `marketplace.routine.${string}` | "marketplace.npx_skills.import.request";
	}
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

const IntentFingerprint = (intent: unknown) =>
	createHash("sha256").update(JSON.stringify(intent)).digest("hex");

export const MakeRoutineMutationHandler = Effect.gen(function* () {
	const routines = yield* RoutineService;
	const repository = yield* RoutineRepository;
	const response = yield* MakeMarketplaceResponse;

	const RequireScope = <Success, Error>(
		routine_id: string,
		scope: MarketplaceScope,
		program: Effect.Effect<Success, Error>,
	) =>
		repository.ReadDetail(routine_id).pipe(
			Effect.filterOrFail(
				(detail) => ScopeMatches(detail.scope, scope),
				() => "Routine is outside the requested Marketplace scope",
			),
			Effect.andThen(program),
		);

	const Handle = (envelope: RoutineMutationEnvelope, current: ReadyState) => {
		switch (envelope.kind) {
			case "marketplace.routine.install.request":
				return response.Action(
					envelope,
					routines.RequestInstall({
						...envelope.payload,
						operation_id: envelope.message_id,
						request_fingerprint: envelope.message_id,
					}),
				);
			case "marketplace.routine.install.decision":
				return response.Action(
					envelope,
					repository.ReadPendingInstall(envelope.payload.approval_id).pipe(
						Effect.filterOrFail(
							(request) =>
								request.approval_fingerprint ===
								envelope.payload.preview_fingerprint,
							() =>
								"Routine approval fingerprint does not match the reviewed request",
						),
						Effect.flatMap((request) =>
							routines.DecideInstall({
								approval_id: request.approval_id,
								approved: envelope.payload.approved,
								operation_id: request.operation_id,
								preview_fingerprint: request.approval_fingerprint,
								request_fingerprint: request.request_fingerprint,
								requested_by: "user",
								scope: request.preview.scope,
								source: request.preview.source,
							}),
						),
					),
				);
			case "marketplace.routine.invoke":
				return RequireScope(
					envelope.payload.routine_id,
					envelope.payload.scope,
					routines.Invoke({
						...envelope.payload,
						engine_id: "codex",
						operation_id: envelope.message_id,
					}),
				).pipe(
					Effect.flatMap((payload) =>
						response.Result(
							envelope,
							current,
							"marketplace.routine.invoke.result",
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
			case "marketplace.routine.drift.overwrite.request": {
				const { intent_fingerprint, ...intent } = envelope.payload;
				return response.Action(
					envelope,
					RequireScope(
						envelope.payload.routine_id,
						envelope.payload.scope,
						Effect.gen(function* () {
							if (IntentFingerprint(intent) !== intent_fingerprint)
								return yield* Effect.fail(
									"Routine drift overwrite intent fingerprint is invalid",
								);
							return yield* repository.RecordPendingDriftOverwrite({
								operation_id: envelope.message_id,
								request: envelope.payload,
							});
						}),
					),
				);
			}
			case "marketplace.routine.drift.overwrite.decision":
				return response.Action(
					envelope,
					repository.ReadPendingDriftOverwrite(envelope.payload.approval_id).pipe(
						Effect.filterOrFail(
							(record) => {
								const request = record.request;
								return (
									request.intent_fingerprint ===
										envelope.payload.intent_fingerprint &&
									request.engine_id === envelope.payload.engine_id &&
									request.observed_revision ===
										envelope.payload.observed_revision &&
									request.routine_id === envelope.payload.routine_id &&
									ScopeMatches(request.scope, envelope.payload.scope)
								);
							},
							() =>
								"Routine drift overwrite decision does not match the reviewed intent",
						),
						Effect.flatMap((record) =>
							RequireScope(
								record.request.routine_id,
								envelope.payload.scope,
								repository
									.DecideDriftOverwrite({
										approval_id: envelope.payload.approval_id,
										approved: envelope.payload.approved,
										intent_fingerprint: envelope.payload.intent_fingerprint,
									})
									.pipe(
										Effect.flatMap((decision) =>
											decision === "denied"
												? Effect.void
												: routines.ExecuteApprovedDriftOverwrite({
														engine_id: record.request.engine_id,
														observed_revision:
															record.request.observed_revision,
														operation_id: envelope.message_id,
														routine_id: record.request.routine_id,
													}),
										),
									),
							),
						),
					),
				);
			case "marketplace.routine.enable":
				return response.Action(
					envelope,
					RequireScope(
						envelope.payload.id,
						envelope.payload.scope,
						routines.Enable({
							operation_id: envelope.message_id,
							routine_id: envelope.payload.id,
						}),
					),
				);
			case "marketplace.routine.disable":
				return response.Action(
					envelope,
					RequireScope(
						envelope.payload.id,
						envelope.payload.scope,
						routines.Disable({
							operation_id: envelope.message_id,
							routine_id: envelope.payload.id,
						}),
					),
				);
			case "marketplace.routine.remove":
				return response.Action(
					envelope,
					RequireScope(
						envelope.payload.id,
						envelope.payload.scope,
						routines.Remove({
							operation_id: envelope.message_id,
							routine_id: envelope.payload.id,
						}),
					),
				);
			case "marketplace.routine.sync":
				return response.Action(
					envelope,
					RequireScope(
						envelope.payload.id,
						envelope.payload.scope,
						routines.Sync({
							engine_id: envelope.payload.engine_id,
							operation_id: envelope.message_id,
							routine_id: envelope.payload.id,
						}),
					),
				);
			case "marketplace.routine.drift.resolve":
				return response.Action(
					envelope,
					RequireScope(
						envelope.payload.routine_id,
						envelope.payload.scope,
						routines.ResolveDrift({
							...envelope.payload,
							operation_id: envelope.message_id,
						}),
					),
				);
			case "marketplace.routine.rollback":
				return response.Action(
					envelope,
					RequireScope(
						envelope.payload.routine_id,
						envelope.payload.scope,
						routines.Rollback({
							operation_id: envelope.message_id,
							rollback_id: envelope.payload.rollback_id,
							routine_id: envelope.payload.routine_id,
						}),
					),
				);
			case "marketplace.npx_skills.import.request":
				return response.Action(
					envelope,
					routines.PreviewNpxImport(envelope.payload).pipe(
						Effect.flatMap((preview) =>
							routines.RequestInstall({
								approval_id: envelope.message_id,
								operation_id: envelope.message_id,
								preview_fingerprint: preview.preview_fingerprint,
								request_fingerprint: envelope.message_id,
								requested_by: "user",
								scope: preview.scope,
								source: preview.source,
							}),
						),
					),
				);
		}
		return Effect.die(new Error(`Unsupported routine mutation: ${envelope.kind}`));
	};

	return Handle;
});
