import { Effect } from "effect";
import {
	type NpxSkillsDiscoverEnvelope,
	type NpxSkillsImportEnvelope,
	type RoutineApprovalDecisionEnvelope,
	type RoutineDetailQueryEnvelope,
	type RoutineDisableEnvelope,
	type RoutineDriftOverwriteDecisionEnvelope,
	type RoutineDriftOverwriteRequestEnvelope,
	type RoutineDriftResolutionEnvelope,
	type RoutineEnableEnvelope,
	type RoutineInstallPreviewEnvelope,
	type RoutineInstallPreviewRequest,
	type RoutineInstallRequestEnvelope,
	type RoutineInvokeEnvelope,
	type RoutineRegistryQueryEnvelope,
	type RoutineRemoveEnvelope,
	type RoutineRollbackEnvelope,
	type RoutineSyncEnvelope,
} from "@artisan/protocol";
import type {
	ArtisanCommandReceipt,
	ArtisanMarketplaceBrowseInput,
	ArtisanNpxSkillsDiscoverInput,
	ArtisanNpxSkillsImportInput,
	ArtisanRoutineApprovalInput,
	ArtisanRoutineDetailInput,
	ArtisanRoutineDriftInput,
	ArtisanRoutineDriftOverwriteDecisionInput,
	ArtisanRoutineDriftOverwriteRequestInput,
	ArtisanRoutineIdInput,
	ArtisanRoutineInstallInput,
	ArtisanRoutineInvokeInput,
	ArtisanRoutineRollbackInput,
	ArtisanRoutineSyncInput,
} from "../../client-api/service";
import { client_error } from "../client-common";
import { ClientApiContext } from "./context";

type RoutineReceiptEnvelope =
	| RoutineApprovalDecisionEnvelope
	| RoutineDisableEnvelope
	| RoutineDriftOverwriteDecisionEnvelope
	| RoutineDriftOverwriteRequestEnvelope
	| RoutineDriftResolutionEnvelope
	| RoutineEnableEnvelope
	| RoutineInstallRequestEnvelope
	| RoutineRemoveEnvelope
	| RoutineRollbackEnvelope
	| RoutineSyncEnvelope
	| NpxSkillsImportEnvelope;

export const MakeRoutineApi = Effect.gen(function* () {
	const context = yield* ClientApiContext;
	const send = (envelope: RoutineReceiptEnvelope) =>
		Effect.gen(function* () {
			const result = yield* context.Request(envelope);
			if (result.kind !== "command.receipt")
				return yield* Effect.die("marketplace receipt narrowed incorrectly");
			if (result.payload.status === "rejected") {
				return yield* Effect.fail(
					client_error(
						"protocol",
						result.payload.error.message,
						result.payload.error,
						result.payload.error.retryable,
						result.payload.error.code,
					),
				);
			}
			return {
				command_id: envelope.message_id,
				journal_sequence: result.payload.journal_sequence,
				status: result.payload.status,
			} satisfies ArtisanCommandReceipt;
		});
	const list_routines = (input: ArtisanMarketplaceBrowseInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: RoutineRegistryQueryEnvelope = {
				...trace,
				kind: "marketplace.routine.list.query",
				payload: input,
			};
			const result = yield* context.Request(envelope);
			return result.kind === "marketplace.routine.list.query.result"
				? result.payload
				: yield* Effect.die("routine registry response narrowed incorrectly");
		});
	const get_routine_detail = (input: ArtisanRoutineDetailInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: RoutineDetailQueryEnvelope = {
				...trace,
				kind: "marketplace.routine.detail.query",
				payload: input,
			};
			const result = yield* context.Request(envelope);
			return result.kind === "marketplace.routine.detail.query.result"
				? result.payload
				: yield* Effect.die("routine detail response narrowed incorrectly");
		});
	const preview_routine_install = (input: RoutineInstallPreviewRequest) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: RoutineInstallPreviewEnvelope = {
				...trace,
				kind: "marketplace.routine.install.preview",
				payload: input,
			};
			const result = yield* context.Request(envelope);
			return result.kind === "marketplace.routine.install.preview.result"
				? result.payload
				: yield* Effect.die("routine install preview response narrowed incorrectly");
		});
	const discover_npx_skills = (input: ArtisanNpxSkillsDiscoverInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: NpxSkillsDiscoverEnvelope = {
				...trace,
				kind: "marketplace.npx_skills.discover",
				payload: input,
			};
			const result = yield* context.Request(envelope);
			return result.kind === "marketplace.npx_skills.discover.result"
				? result.payload
				: yield* Effect.die("npx skills discovery response narrowed incorrectly");
		});
	const routine_lifecycle = (
		input: ArtisanRoutineIdInput,
		kind:
			| "marketplace.routine.enable"
			| "marketplace.routine.disable"
			| "marketplace.routine.remove",
	) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: RoutineEnableEnvelope | RoutineDisableEnvelope | RoutineRemoveEnvelope =
				{
					...trace,
					message_id: input.command_id ?? trace.message_id,
					kind,
					payload: { id: input.routine_id, scope: input.scope },
				};
			return yield* send(envelope);
		});
	const request_routine_install = (input: ArtisanRoutineInstallInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send({
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.routine.install.request",
				payload: {
					approval_id: input.approval_id,
					preview_fingerprint: input.preview_fingerprint,
					requested_by: input.requested_by,
					scope: input.scope,
					source: input.source,
				},
			});
		});
	const decide_routine_install = (input: ArtisanRoutineApprovalInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send({
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.routine.install.decision",
				payload: {
					approval_id: input.approval_id,
					approved: input.approved,
					preview_fingerprint: input.preview_fingerprint,
				},
			});
		});
	const sync_routine = (input: ArtisanRoutineSyncInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send({
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.routine.sync",
				payload: { engine_id: input.engine_id, id: input.id, scope: input.scope },
			});
		});
	const resolve_routine_drift = (input: ArtisanRoutineDriftInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send({
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.routine.drift.resolve",
				payload: {
					action: input.action,
					engine_id: input.engine_id,
					observed_revision: input.observed_revision,
					routine_id: input.routine_id,
					scope: input.scope,
				},
			});
		});
	const request_routine_drift_overwrite = (input: ArtisanRoutineDriftOverwriteRequestInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send({
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.routine.drift.overwrite.request",
				payload: {
					approval_id: input.approval_id,
					engine_id: input.engine_id,
					intent_fingerprint: input.intent_fingerprint,
					observed_revision: input.observed_revision,
					requested_by: input.requested_by,
					routine_id: input.routine_id,
					scope: input.scope,
				},
			});
		});
	const decide_routine_drift_overwrite = (input: ArtisanRoutineDriftOverwriteDecisionInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send({
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.routine.drift.overwrite.decision",
				payload: {
					approval_id: input.approval_id,
					approved: input.approved,
					engine_id: input.engine_id,
					intent_fingerprint: input.intent_fingerprint,
					observed_revision: input.observed_revision,
					routine_id: input.routine_id,
					scope: input.scope,
				},
			});
		});
	const rollback_routine = (input: ArtisanRoutineRollbackInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send({
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.routine.rollback",
				payload: {
					rollback_id: input.rollback_id,
					routine_id: input.routine_id,
					scope: input.scope,
				},
			});
		});
	const import_npx_skills = (input: ArtisanNpxSkillsImportInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send({
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.npx_skills.import.request",
				payload: {
					candidate_name: input.candidate_name,
					package_spec: input.package_spec,
					preview_fingerprint: input.preview_fingerprint,
					scope: input.scope,
				},
			});
		});
	const invoke_routine = (input: ArtisanRoutineInvokeInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: RoutineInvokeEnvelope = {
				...trace,
				kind: "marketplace.routine.invoke",
				payload: input,
			};
			const result = yield* context.Request(envelope);
			return result.kind === "marketplace.routine.invoke.result"
				? result.payload
				: yield* Effect.die("routine invocation response narrowed incorrectly");
		});

	return {
		decide_routine_drift_overwrite,
		decide_routine_install,
		discover_npx_skills,
		get_routine_detail,
		import_npx_skills,
		invoke_routine,
		list_routines,
		preview_routine_install,
		request_routine_drift_overwrite,
		request_routine_install,
		resolve_routine_drift,
		rollback_routine,
		routine_lifecycle,
		sync_routine,
	};
});
