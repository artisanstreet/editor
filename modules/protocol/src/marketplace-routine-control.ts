import { Schema } from "effect";

import { Identifier, PositiveInt } from "./common";
import {
	marketplace_collection_maximum_items,
	MarketplaceArtifactIdentity,
	MarketplaceEngine,
	MarketplaceEngineSync,
	MarketplaceScope,
	Routine,
	RoutineInstallApproval,
	RoutineInstallCandidate,
	RoutineInstructions,
} from "./marketplace";

const StableIdentifier = Identifier.check(Schema.isMaxLength(256));
const Fingerprint = Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/));

const same_source = (
	left: MarketplaceArtifactIdentity["source"],
	right: MarketplaceArtifactIdentity["source"],
) =>
	left.display_name === right.display_name &&
	left.kind === right.kind &&
	left.locator === right.locator &&
	left.revision === right.revision;

const same_identity = (left: MarketplaceArtifactIdentity, right: MarketplaceArtifactIdentity) =>
	left.version === right.version && same_source(left.source, right.source);

const same_scope = (left: MarketplaceScope, right: MarketplaceScope) =>
	left.kind === right.kind &&
	(left.kind === "global" ||
		(right.kind !== "global" &&
			(left.kind === "workspace"
				? right.kind === "workspace" && left.workspace_id === right.workspace_id
				: right.kind === "project" && left.project_id === right.project_id)));

const same_values = <A>(
	left: ReadonlyArray<A>,
	right: ReadonlyArray<A>,
	equals: (left: A, right: A) => boolean,
) => left.length === right.length && left.every((value, index) => equals(value, right[index]!));

const same_candidate_and_routine = (candidate: RoutineInstallCandidate, routine: Routine) =>
	candidate.display_name === routine.display_name &&
	candidate.instructions.content_hash === routine.instructions.content_hash &&
	same_scope(candidate.scope, routine.scope) &&
	candidate.summary.description === routine.summary.description &&
	candidate.summary.display_name === routine.summary.display_name &&
	candidate.summary.routine_id === routine.summary.routine_id &&
	same_identity(candidate.summary.identity, routine.summary.identity) &&
	candidate.trust.level === routine.trust.level &&
	same_values(candidate.compatibility, routine.compatibility, (left, right) => left === right) &&
	same_values(candidate.trust.reasons, routine.trust.reasons, (left, right) => left === right) &&
	same_values(
		candidate.commands,
		routine.commands,
		(left, right) =>
			left.command_id === right.command_id &&
			left.description === right.description &&
			left.label === right.label,
	) &&
	same_values(
		candidate.files,
		routine.files,
		(left, right) =>
			left.path === right.path &&
			left.purpose === right.purpose &&
			left.write_mode === right.write_mode,
	) &&
	same_values(
		candidate.permissions,
		routine.permissions,
		(left, right) =>
			left.kind === right.kind &&
			left.label === right.label &&
			left.required === right.required,
	);

const same_sync = (left: MarketplaceEngineSync, right: MarketplaceEngineSync) =>
	left.drift === right.drift &&
	left.engine === right.engine &&
	left.last_error_code === right.last_error_code &&
	left.status === right.status &&
	left.updated_at === right.updated_at &&
	same_identity(left.identity, right.identity);

/** Carries one exact installed Routine snapshot used to fence every mutation. */
export const RoutineInstallation = Schema.Struct({
	installation_id: StableIdentifier,
	routine: Routine,
});

export type RoutineInstallation = typeof RoutineInstallation.Type;

/** References one installed Routine without carrying its full source-safe projection. */
export const RoutineInstallationReference = Schema.Struct({
	identity: MarketplaceArtifactIdentity,
	installation_id: StableIdentifier,
	routine_id: StableIdentifier,
	scope: MarketplaceScope,
});

export type RoutineInstallationReference = typeof RoutineInstallationReference.Type;

const installation_matches_reference = (
	installation: RoutineInstallation,
	reference: RoutineInstallationReference,
) =>
	installation.installation_id === reference.installation_id &&
	installation.routine.summary.routine_id === reference.routine_id &&
	same_identity(installation.routine.summary.identity, reference.identity) &&
	same_scope(installation.routine.scope, reference.scope);

/** Selects the canonical scope and engine used to determine Routine eligibility. */
export const RoutineEligibilityContext = Schema.Struct({
	engine: MarketplaceEngine,
	scope: MarketplaceScope,
});

export type RoutineEligibilityContext = typeof RoutineEligibilityContext.Type;

/** Binds an enabled compatible Routine installation to one eligible engine result. */
const EligibleRoutineUnchecked = Schema.Struct({
	engine: MarketplaceEngine,
	installation: RoutineInstallation,
	state: Schema.Literal("eligible"),
});

type EligibleRoutineUnchecked = typeof EligibleRoutineUnchecked.Type;

export const EligibleRoutine = EligibleRoutineUnchecked.check(
	Schema.makeFilter<EligibleRoutineUnchecked>((eligible) =>
		eligible.installation.routine.lifecycle === "enabled" &&
		eligible.installation.routine.compatibility.includes(eligible.engine)
			? undefined
			: "Expected an enabled Routine installation compatible with the eligible engine",
	),
);

export type EligibleRoutine = typeof EligibleRoutine.Type;

const eligible_matches_context = (eligible: EligibleRoutine, context: RoutineEligibilityContext) =>
	eligible.engine === context.engine &&
	same_scope(eligible.installation.routine.scope, context.scope);

/** Requests installed Routines eligible for one canonical scope and engine. */
export const RoutineListQuery = Schema.Struct({ context: RoutineEligibilityContext });

export type RoutineListQuery = typeof RoutineListQuery.Type;

/** Returns bounded eligible Routine snapshots without full instruction content. */
export const RoutineListResult = Schema.Struct({
	query: RoutineListQuery,
	routines: Schema.Array(EligibleRoutine).check(
		Schema.isMaxLength(marketplace_collection_maximum_items),
	),
}).check(
	Schema.makeFilter<{
		readonly query: RoutineListQuery;
		readonly routines: ReadonlyArray<EligibleRoutine>;
	}>((result) => {
		const installation_ids = result.routines.map(
			({ installation }) => installation.installation_id,
		);
		const routine_ids = result.routines.map(
			({ installation }) => installation.routine.summary.routine_id,
		);

		if (new Set(installation_ids).size !== installation_ids.length) {
			return "Expected unique eligible Routine installation identifiers";
		}

		if (new Set(routine_ids).size !== routine_ids.length) {
			return "Expected unique eligible Routine identifiers";
		}

		return result.routines.every((eligible) =>
			eligible_matches_context(eligible, result.query.context),
		)
			? undefined
			: "Expected every listed Routine to match the requested scope and engine";
	}),
);

export type RoutineListResult = typeof RoutineListResult.Type;

/** Requests one installed Routine under an exact scope and engine eligibility context. */
export const RoutineReadQuery = Schema.Struct({
	context: RoutineEligibilityContext,
	routine: RoutineInstallationReference,
}).check(
	Schema.makeFilter<{
		readonly context: RoutineEligibilityContext;
		readonly routine: RoutineInstallationReference;
	}>((query) =>
		same_scope(query.context.scope, query.routine.scope)
			? undefined
			: "Expected the Routine read scope to match its installation reference",
	),
);

export type RoutineReadQuery = typeof RoutineReadQuery.Type;

/** Returns one optional eligible Routine snapshot without full instruction content. */
export const RoutineReadResult = Schema.Struct({
	query: RoutineReadQuery,
	routine: Schema.optional(EligibleRoutine),
}).check(
	Schema.makeFilter<{
		readonly query: RoutineReadQuery;
		readonly routine?: EligibleRoutine | undefined;
	}>((result) =>
		result.routine === undefined ||
		(eligible_matches_context(result.routine, result.query.context) &&
			installation_matches_reference(result.routine.installation, result.query.routine))
			? undefined
			: "Expected the read result to match the requested installation, scope, and engine",
	),
);

export type RoutineReadResult = typeof RoutineReadResult.Type;

/** Requests full instructions for one exact eligible Routine snapshot. */
export const RoutineInstructionsQuery = Schema.Struct({ eligibility: EligibleRoutine });

export type RoutineInstructionsQuery = typeof RoutineInstructionsQuery.Type;

/** Returns full instructions only for the exact hash-bound Routine query. */
export const RoutineInstructionsResult = Schema.Struct({
	instructions: RoutineInstructions,
	query: RoutineInstructionsQuery,
}).check(
	Schema.makeFilter<{
		readonly instructions: RoutineInstructions;
		readonly query: RoutineInstructionsQuery;
	}>((result) =>
		result.instructions.content_hash ===
		result.query.eligibility.installation.routine.instructions.content_hash
			? undefined
			: "Expected full Routine instructions to match the selected content hash",
	),
);

export type RoutineInstructionsResult = typeof RoutineInstructionsResult.Type;

/** Requests one complete candidate preview under a stable preview operation. */
export const RoutineInstallPreviewRequest = Schema.Struct({
	candidate: RoutineInstallCandidate,
	preview_operation_id: StableIdentifier,
});

export type RoutineInstallPreviewRequest = typeof RoutineInstallPreviewRequest.Type;

/** Records an exact-replay decision for one pending, immutable Routine preview. */
export const RoutineInstallDecisionRequest = Schema.Struct({
	approval: RoutineInstallApproval,
	approval_id: StableIdentifier,
	decision: Schema.Literals(["approved", "denied"]),
	decision_id: StableIdentifier,
	operation_id: StableIdentifier,
	preview_operation_id: StableIdentifier,
}).check(
	Schema.makeFilter<{
		readonly approval: RoutineInstallApproval;
		readonly approval_id: string;
		readonly preview_operation_id: string;
	}>((request) =>
		request.approval.decision === "pending" &&
		request.approval_id === request.approval.approval_id &&
		request.preview_operation_id === request.approval.preview.preview_operation_id &&
		request.preview_operation_id === request.approval.preview_operation_id
			? undefined
			: "Expected a pending approval bound to the exact preview operation",
	),
);

export type RoutineInstallDecisionRequest = typeof RoutineInstallDecisionRequest.Type;

/** Requests irreversible installation of one exact approved candidate snapshot. */
export const RoutineInstallRequest = Schema.Struct({
	approval: RoutineInstallApproval,
	approval_id: StableIdentifier,
	installation_id: StableIdentifier,
	operation_id: StableIdentifier,
	preview_operation_id: StableIdentifier,
	scope: MarketplaceScope,
}).check(
	Schema.makeFilter<{
		readonly approval: RoutineInstallApproval;
		readonly approval_id: string;
		readonly installation_id: string;
		readonly preview_operation_id: string;
		readonly scope: MarketplaceScope;
	}>((request) => {
		const preview = request.approval.preview;

		return request.approval.decision === "approved" &&
			request.approval_id === request.approval.approval_id &&
			request.preview_operation_id === request.approval.preview_operation_id &&
			request.preview_operation_id === preview.preview_operation_id &&
			request.installation_id === preview.rollback.installation_id &&
			same_scope(request.scope, preview.candidate.scope) &&
			same_scope(request.scope, preview.rollback.scope)
			? undefined
			: "Expected installation to retain the exact approved preview, installation, and scope";
	}),
);

export type RoutineInstallRequest = typeof RoutineInstallRequest.Type;

const RoutineLifecycleRequestFields = {
	installation: RoutineInstallation,
	operation_id: StableIdentifier,
};

const lifecycle_matches = (
	installation: RoutineInstallation,
	expected_lifecycle: "enabled" | "disabled",
) => installation.routine.lifecycle === expected_lifecycle;

/** Requests the legal enabled-to-disabled Routine lifecycle transition. */
export const RoutineEnabledToDisabledRequest = Schema.Struct({
	...RoutineLifecycleRequestFields,
	transition: Schema.Literal("enabled_to_disabled"),
}).check(
	Schema.makeFilter<{ readonly installation: RoutineInstallation }>((request) =>
		lifecycle_matches(request.installation, "enabled")
			? undefined
			: "Expected an enabled Routine before disabling it",
	),
);

/** Requests the legal enabled-to-removed Routine lifecycle transition. */
export const RoutineEnabledToRemovedRequest = Schema.Struct({
	...RoutineLifecycleRequestFields,
	transition: Schema.Literal("enabled_to_removed"),
}).check(
	Schema.makeFilter<{ readonly installation: RoutineInstallation }>((request) =>
		lifecycle_matches(request.installation, "enabled")
			? undefined
			: "Expected an enabled Routine before removing it",
	),
);

/** Requests the legal disabled-to-enabled Routine lifecycle transition. */
export const RoutineDisabledToEnabledRequest = Schema.Struct({
	...RoutineLifecycleRequestFields,
	transition: Schema.Literal("disabled_to_enabled"),
}).check(
	Schema.makeFilter<{ readonly installation: RoutineInstallation }>((request) =>
		lifecycle_matches(request.installation, "disabled")
			? undefined
			: "Expected a disabled Routine before enabling it",
	),
);

/** Requests the legal disabled-to-removed Routine lifecycle transition. */
export const RoutineDisabledToRemovedRequest = Schema.Struct({
	...RoutineLifecycleRequestFields,
	transition: Schema.Literal("disabled_to_removed"),
}).check(
	Schema.makeFilter<{ readonly installation: RoutineInstallation }>((request) =>
		lifecycle_matches(request.installation, "disabled")
			? undefined
			: "Expected a disabled Routine before removing it",
	),
);

/** Encodes every legal non-removed Routine lifecycle transition. */
export const RoutineLifecycleRequest = Schema.Union([
	RoutineEnabledToDisabledRequest,
	RoutineEnabledToRemovedRequest,
	RoutineDisabledToEnabledRequest,
	RoutineDisabledToRemovedRequest,
]);

export type RoutineLifecycleRequest = typeof RoutineLifecycleRequest.Type;

/** References one immutable stored rollback plan without accepting mutable actions. */
export const RoutineRollbackPlanReference = Schema.Struct({
	identity: MarketplaceArtifactIdentity,
	installation_id: StableIdentifier,
	plan_fingerprint: Fingerprint,
	plan_version: PositiveInt,
	rollback_id: StableIdentifier,
	scope: MarketplaceScope,
});

export type RoutineRollbackPlanReference = typeof RoutineRollbackPlanReference.Type;

const rollback_plan_matches_reference = (
	plan: RoutineInstallApproval["preview"]["rollback"],
	reference: RoutineRollbackPlanReference,
) =>
	plan.installation_id === reference.installation_id &&
	plan.plan_fingerprint === reference.plan_fingerprint &&
	plan.plan_version === reference.plan_version &&
	plan.rollback_id === reference.rollback_id &&
	same_identity(plan.identity, reference.identity) &&
	same_scope(plan.scope, reference.scope);

/** Requests rollback of the exact stored plan, installation, preview, and approval context. */
export const RoutineRollbackRequest = Schema.Struct({
	approval: RoutineInstallApproval,
	approval_id: StableIdentifier,
	installation: RoutineInstallation,
	operation_id: StableIdentifier,
	plan: RoutineRollbackPlanReference,
	preview_operation_id: StableIdentifier,
}).check(
	Schema.makeFilter<{
		readonly approval: RoutineInstallApproval;
		readonly approval_id: string;
		readonly installation: RoutineInstallation;
		readonly plan: RoutineRollbackPlanReference;
		readonly preview_operation_id: string;
	}>((request) =>
		(request.approval.decision === "applied" || request.approval.decision === "failed") &&
		request.approval_id === request.approval.approval_id &&
		request.preview_operation_id === request.approval.preview_operation_id &&
		request.preview_operation_id === request.approval.preview.preview_operation_id &&
		request.approval.preview.rollback.available &&
		rollback_plan_matches_reference(request.approval.preview.rollback, request.plan) &&
		request.installation.installation_id === request.plan.installation_id &&
		same_identity(request.installation.routine.summary.identity, request.plan.identity) &&
		same_scope(request.installation.routine.scope, request.plan.scope) &&
		same_candidate_and_routine(request.approval.preview.candidate, request.installation.routine)
			? undefined
			: "Expected rollback to reference the exact stored plan, approval, and installed Routine snapshot",
	),
);

export type RoutineRollbackRequest = typeof RoutineRollbackRequest.Type;

const mirror_request_error = (request: {
	readonly expected_sync: MarketplaceEngineSync;
	readonly installation: RoutineInstallation;
}) => {
	const routine = request.installation.routine;

	if (routine.lifecycle === "removed") {
		return "Expected a non-removed Routine installation for mirror control";
	}

	if (!routine.compatibility.includes(request.expected_sync.engine)) {
		return "Expected the mirror engine to be compatible with the installed Routine";
	}

	return routine.sync.some((sync) => same_sync(sync, request.expected_sync))
		? undefined
		: "Expected the complete current mirror row from the installed Routine snapshot";
};

/** Resolves current provider mirror drift against one exact installed Routine snapshot. */
export const RoutineMirrorDriftResolutionRequest = Schema.Struct({
	expected_sync: MarketplaceEngineSync,
	installation: RoutineInstallation,
	operation_id: StableIdentifier,
	resolution: Schema.Literals(["adopt_mirror", "replace_mirror"]),
}).check(
	Schema.makeFilter<{
		readonly expected_sync: MarketplaceEngineSync;
		readonly installation: RoutineInstallation;
	}>((request) =>
		mirror_request_error(request) === undefined &&
		request.expected_sync.status === "drift_detected" &&
		(request.expected_sync.drift === "detected" ||
			request.expected_sync.drift === "resolution_required")
			? undefined
			: "Expected the exact current drifted mirror row for resolution",
	),
);

export type RoutineMirrorDriftResolutionRequest = typeof RoutineMirrorDriftResolutionRequest.Type;

/** Retries one current failed provider mirror operation against its exact installed snapshot. */
export const RoutineMirrorRetryRequest = Schema.Struct({
	expected_sync: MarketplaceEngineSync,
	installation: RoutineInstallation,
	operation_id: StableIdentifier,
}).check(
	Schema.makeFilter<{
		readonly expected_sync: MarketplaceEngineSync;
		readonly installation: RoutineInstallation;
	}>((request) =>
		mirror_request_error(request) === undefined &&
		request.expected_sync.status === "sync_failed"
			? undefined
			: "Expected the exact current failed mirror row for retry",
	),
);

export type RoutineMirrorRetryRequest = typeof RoutineMirrorRetryRequest.Type;

/** Attributes one Routine invocation to its engine agent run and thread. */
export const RoutineInvocationContext = Schema.Struct({
	agent_id: StableIdentifier,
	run_id: StableIdentifier,
	thread_id: StableIdentifier,
});

export type RoutineInvocationContext = typeof RoutineInvocationContext.Type;

/** Requests one command from an exact eligible Routine without arguments or results. */
export const EligibleRoutineInvocationRequest = Schema.Struct({
	command_id: StableIdentifier,
	context: RoutineInvocationContext,
	eligibility: EligibleRoutine,
	invocation_id: StableIdentifier,
}).check(
	Schema.makeFilter<{
		readonly command_id: string;
		readonly eligibility: EligibleRoutine;
	}>((request) =>
		request.eligibility.installation.routine.commands.some(
			(command) => command.command_id === request.command_id,
		)
			? undefined
			: "Expected the invocation command on the exact eligible Routine snapshot",
	),
);

export type EligibleRoutineInvocationRequest = typeof EligibleRoutineInvocationRequest.Type;

const strict_decoder = <S extends Schema.Constraint>(schema: S) => {
	const Decode = Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" });

	return (input: unknown) => Decode(input);
};

/** Strictly decodes one installed Routine snapshot. */
export const DecodeRoutineInstallation = strict_decoder(RoutineInstallation);

/** Strictly decodes one Routine list query. */
export const DecodeRoutineListQuery = strict_decoder(RoutineListQuery);

/** Strictly decodes one Routine list result. */
export const DecodeRoutineListResult = strict_decoder(RoutineListResult);

/** Strictly decodes one Routine read query. */
export const DecodeRoutineReadQuery = strict_decoder(RoutineReadQuery);

/** Strictly decodes one Routine read result. */
export const DecodeRoutineReadResult = strict_decoder(RoutineReadResult);

/** Strictly decodes one explicit Routine instructions query. */
export const DecodeRoutineInstructionsQuery = strict_decoder(RoutineInstructionsQuery);

/** Strictly decodes one explicit Routine instructions result. */
export const DecodeRoutineInstructionsResult = strict_decoder(RoutineInstructionsResult);

/** Strictly decodes one Routine install-preview request. */
export const DecodeRoutineInstallPreviewRequest = strict_decoder(RoutineInstallPreviewRequest);

/** Strictly decodes one Routine installation approval decision. */
export const DecodeRoutineInstallDecisionRequest = strict_decoder(RoutineInstallDecisionRequest);

/** Strictly decodes one approval-bound Routine install request. */
export const DecodeRoutineInstallRequest = strict_decoder(RoutineInstallRequest);

/** Strictly decodes one legal Routine lifecycle transition. */
export const DecodeRoutineLifecycleRequest = strict_decoder(RoutineLifecycleRequest);

/** Strictly decodes one exact stored-plan Routine rollback request. */
export const DecodeRoutineRollbackRequest = strict_decoder(RoutineRollbackRequest);

/** Strictly decodes one exact provider mirror-drift resolution request. */
export const DecodeRoutineMirrorDriftResolutionRequest = strict_decoder(
	RoutineMirrorDriftResolutionRequest,
);

/** Strictly decodes one exact provider mirror retry request. */
export const DecodeRoutineMirrorRetryRequest = strict_decoder(RoutineMirrorRetryRequest);

/** Strictly decodes one eligible Routine invocation without private payloads. */
export const DecodeEligibleRoutineInvocationRequest = strict_decoder(
	EligibleRoutineInvocationRequest,
);
