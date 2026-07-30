import { Effect } from "effect";
import {
	type CapabilityApprovalDecisionEnvelope,
	type CapabilityConnectPreviewEnvelope,
	type CapabilityConnectPreviewRequest,
	type CapabilityConnectRequestEnvelope,
	type CapabilityDetailQueryEnvelope,
	type CapabilityDisableEnvelope,
	type CapabilityDisconnectEnvelope,
	type CapabilityDriftOverwriteDecisionEnvelope,
	type CapabilityDriftOverwriteRequestEnvelope,
	type CapabilityDriftResolutionEnvelope,
	type CapabilityEnableEnvelope,
	type CapabilityHealthEnvelope,
	type CapabilityInvokeEnvelope,
	type CapabilityOAuthBeginEnvelope,
	type CapabilityOAuthCompleteEnvelope,
	type CapabilityOAuthRefreshEnvelope,
	type CapabilityOAuthRevokeEnvelope,
	type CapabilityOAuthTokenStatusEnvelope,
	type CapabilityReconnectEnvelope,
	type CapabilityRegistryQueryEnvelope,
	type CapabilityRemoveEnvelope,
	type CapabilityRestartEnvelope,
	type CapabilityStartEnvelope,
	type CapabilitySyncEnvelope,
	type CapabilityUninstallEnvelope,
} from "@artisan/protocol";
import type {
	ArtisanCapabilityApprovalInput,
	ArtisanCapabilityConnectInput,
	ArtisanCapabilityDetailInput,
	ArtisanCapabilityDriftInput,
	ArtisanCapabilityDriftOverwriteDecisionInput,
	ArtisanCapabilityDriftOverwriteRequestInput,
	ArtisanCapabilityHealthInput,
	ArtisanCapabilityIdInput,
	ArtisanCapabilityInvocationDecisionInput,
	ArtisanCapabilityInvocationRequestInput,
	ArtisanCapabilityInvokeInput,
	ArtisanCapabilityOAuthCompleteInput,
	ArtisanCapabilityOAuthInput,
	ArtisanCapabilitySyncInput,
	ArtisanCommandReceipt,
	ArtisanMarketplaceBrowseInput,
} from "../../client-api/service";
import { client_error } from "../client-common";
import { ClientApiContext } from "./context";

type CapabilityReceiptEnvelope =
	| CapabilityApprovalDecisionEnvelope
	| CapabilityConnectRequestEnvelope
	| CapabilityDisableEnvelope
	| CapabilityDisconnectEnvelope
	| CapabilityDriftOverwriteDecisionEnvelope
	| CapabilityDriftOverwriteRequestEnvelope
	| CapabilityDriftResolutionEnvelope
	| CapabilityEnableEnvelope
	| CapabilityHealthEnvelope
	| CapabilityOAuthCompleteEnvelope
	| CapabilityOAuthRefreshEnvelope
	| CapabilityOAuthRevokeEnvelope
	| CapabilityReconnectEnvelope
	| CapabilityRemoveEnvelope
	| CapabilityRestartEnvelope
	| CapabilityStartEnvelope
	| CapabilitySyncEnvelope
	| CapabilityUninstallEnvelope;

export const MakeCapabilityApi = Effect.gen(function* () {
	const context = yield* ClientApiContext;
	const send = (envelope: CapabilityReceiptEnvelope) =>
		Effect.gen(function* () {
			const result = yield* context.Request(envelope);
			if (result.kind !== "command.receipt")
				return yield* Effect.die("marketplace receipt narrowed incorrectly");
			if (result.payload.status === "rejected")
				return yield* Effect.fail(
					client_error(
						"protocol",
						result.payload.error.message,
						result.payload.error,
						result.payload.error.retryable,
						result.payload.error.code,
					),
				);
			return {
				command_id: envelope.message_id,
				journal_sequence: result.payload.journal_sequence,
				status: result.payload.status,
			} satisfies ArtisanCommandReceipt;
		});
	const list_capabilities = (input: ArtisanMarketplaceBrowseInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: CapabilityRegistryQueryEnvelope = {
				...trace,
				kind: "marketplace.capability.list.query",
				payload: input,
			};
			const result = yield* context.Request(envelope);
			return result.kind === "marketplace.capability.list.query.result"
				? result.payload
				: yield* Effect.die("capability registry response narrowed incorrectly");
		});
	const get_capability_detail = (input: ArtisanCapabilityDetailInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: CapabilityDetailQueryEnvelope = {
				...trace,
				kind: "marketplace.capability.detail.query",
				payload: input,
			};
			const result = yield* context.Request(envelope);
			return result.kind === "marketplace.capability.detail.query.result"
				? result.payload
				: yield* Effect.die("capability detail response narrowed incorrectly");
		});
	const preview_capability_connect = (input: CapabilityConnectPreviewRequest) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: CapabilityConnectPreviewEnvelope = {
				...trace,
				kind: "marketplace.capability.connect.preview",
				payload: input,
			};
			const result = yield* context.Request(envelope);
			return result.kind === "marketplace.capability.connect.preview.result"
				? result.payload
				: yield* Effect.die("capability connect preview response narrowed incorrectly");
		});
	const capability_lifecycle = (
		input: ArtisanCapabilityIdInput,
		kind:
			| "marketplace.capability.start"
			| "marketplace.capability.reconnect"
			| "marketplace.capability.disconnect"
			| "marketplace.capability.restart"
			| "marketplace.capability.uninstall",
	) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope:
				| CapabilityStartEnvelope
				| CapabilityReconnectEnvelope
				| CapabilityDisconnectEnvelope
				| CapabilityRestartEnvelope
				| CapabilityUninstallEnvelope = {
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind,
				payload: { capability_id: input.capability_id, scope: input.scope },
			};
			return yield* send(envelope);
		});
	const capability_enablement = (
		input: ArtisanCapabilityIdInput,
		kind:
			| "marketplace.capability.enable"
			| "marketplace.capability.disable"
			| "marketplace.capability.remove",
	) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope:
				| CapabilityEnableEnvelope
				| CapabilityDisableEnvelope
				| CapabilityRemoveEnvelope = {
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind,
				payload: { id: input.capability_id, scope: input.scope },
			};
			return yield* send(envelope);
		});
	const request_capability_connect = (input: ArtisanCapabilityConnectInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send({
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.capability.connect.request",
				payload: {
					approval_id: input.approval_id,
					auth: input.auth,
					preview_fingerprint: input.preview_fingerprint,
					requested_by: input.requested_by,
					scope: input.scope,
					source: input.source,
					transport: input.transport,
				},
			});
		});
	const decide_capability_connect = (input: ArtisanCapabilityApprovalInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send({
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.capability.connect.decision",
				payload: {
					approval_id: input.approval_id,
					approved: input.approved,
					preview_fingerprint: input.preview_fingerprint,
				},
			});
		});
	const check_capability_health = (input: ArtisanCapabilityHealthInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send({
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.capability.health",
				payload: { capability_id: input.capability_id, scope: input.scope },
			});
		});
	const sync_capability = (input: ArtisanCapabilitySyncInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send({
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.capability.sync",
				payload: { engine_id: input.engine_id, id: input.id, scope: input.scope },
			});
		});
	const resolve_capability_drift = (input: ArtisanCapabilityDriftInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send({
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.capability.drift.resolve",
				payload: {
					action: input.action,
					capability_id: input.capability_id,
					engine_id: input.engine_id,
					observed_revision: input.observed_revision,
					scope: input.scope,
				},
			});
		});
	const request_capability_drift_overwrite = (
		input: ArtisanCapabilityDriftOverwriteRequestInput,
	) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send({
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.capability.drift.overwrite.request",
				payload: {
					approval_id: input.approval_id,
					capability_id: input.capability_id,
					engine_id: input.engine_id,
					intent_fingerprint: input.intent_fingerprint,
					observed_revision: input.observed_revision,
					requested_by: input.requested_by,
					scope: input.scope,
				},
			});
		});
	const decide_capability_drift_overwrite = (
		input: ArtisanCapabilityDriftOverwriteDecisionInput,
	) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send({
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.capability.drift.overwrite.decision",
				payload: {
					approval_id: input.approval_id,
					approved: input.approved,
					capability_id: input.capability_id,
					engine_id: input.engine_id,
					intent_fingerprint: input.intent_fingerprint,
					observed_revision: input.observed_revision,
					scope: input.scope,
				},
			});
		});
	const request_capability_invocation = (input: ArtisanCapabilityInvocationRequestInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.capability.invoke.request",
				payload: {
					approval_id: input.approval_id,
					arguments_json: input.arguments_json,
					capability_id: input.capability_id,
					intent_fingerprint: input.intent_fingerprint,
					requested_by: input.requested_by,
					scope: input.scope,
					tool_name: input.tool_name,
				},
			});
			return result.kind === "marketplace.capability.invoke.result"
				? result.payload
				: yield* Effect.die("capability invocation request narrowed incorrectly");
		});
	const decide_capability_invocation = (input: ArtisanCapabilityInvocationDecisionInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const result = yield* context.Request({
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.capability.invoke.decision",
				payload: {
					approval_id: input.approval_id,
					approved: input.approved,
					arguments_json: input.arguments_json,
					capability_id: input.capability_id,
					intent_fingerprint: input.intent_fingerprint,
					scope: input.scope,
					tool_name: input.tool_name,
				},
			});
			return result.kind === "marketplace.capability.invoke.result"
				? result.payload
				: yield* Effect.die("capability invocation decision narrowed incorrectly");
		});
	const invoke_capability = (input: ArtisanCapabilityInvokeInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: CapabilityInvokeEnvelope = {
				...trace,
				kind: "marketplace.capability.invoke",
				payload: input,
			};
			const result = yield* context.Request(envelope);
			return result.kind === "marketplace.capability.invoke.result"
				? result.payload
				: yield* Effect.die("capability invocation response narrowed incorrectly");
		});
	const capability_oauth_mutation = (
		input: ArtisanCapabilityOAuthInput,
		kind: "marketplace.capability.oauth.refresh" | "marketplace.capability.oauth.revoke",
	) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: CapabilityOAuthRefreshEnvelope | CapabilityOAuthRevokeEnvelope = {
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind,
				payload: { capability_id: input.capability_id, scope: input.scope },
			};
			return yield* send(envelope);
		});
	const begin_capability_oauth = (input: ArtisanCapabilityOAuthInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: CapabilityOAuthBeginEnvelope = {
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.capability.oauth.begin",
				payload: { capability_id: input.capability_id, scope: input.scope },
			};
			const result = yield* context.Request(envelope);
			return result.kind === "marketplace.capability.oauth.begin.result"
				? result.payload
				: yield* Effect.die("capability OAuth begin response narrowed incorrectly");
		});
	const complete_capability_oauth = (input: ArtisanCapabilityOAuthCompleteInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send({
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "marketplace.capability.oauth.complete",
				payload: {
					callback_reference: input.callback_reference,
					capability_id: input.capability_id,
					scope: input.scope,
				},
			});
		});
	const get_capability_oauth_status = (input: ArtisanCapabilityOAuthInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: CapabilityOAuthTokenStatusEnvelope = {
				...trace,
				kind: "marketplace.capability.oauth.status.query",
				payload: { capability_id: input.capability_id, scope: input.scope },
			};
			const result = yield* context.Request(envelope);
			return result.kind === "marketplace.capability.oauth.status.query.result"
				? result.payload
				: yield* Effect.die("capability OAuth status response narrowed incorrectly");
		});

	return {
		begin_capability_oauth,
		capability_enablement,
		capability_lifecycle,
		capability_oauth_mutation,
		check_capability_health,
		complete_capability_oauth,
		decide_capability_connect,
		decide_capability_drift_overwrite,
		decide_capability_invocation,
		get_capability_detail,
		get_capability_oauth_status,
		invoke_capability,
		list_capabilities,
		preview_capability_connect,
		request_capability_connect,
		request_capability_drift_overwrite,
		request_capability_invocation,
		resolve_capability_drift,
		sync_capability,
	};
});
