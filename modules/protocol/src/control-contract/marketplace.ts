import { Identifier } from "../common";

import {
	CapabilityConnectPreview,
	CapabilityConnectPreviewRequest,
	CapabilityConnectRequest,
	CapabilityDetail,
	CapabilityDriftOverwriteDecision,
	CapabilityDriftOverwriteRequest,
	CapabilityDriftResolutionRequest,
	CapabilityHealthRequest,
	CapabilityInvocationApprovalDecision,
	CapabilityInvocationApprovalRequest,
	CapabilityInvocationMetadata,
	CapabilityInvocationRequest,
	CapabilityLifecycleRequest,
	CapabilityOAuthBeginResult,
	CapabilityOAuthCompleteRequest,
	CapabilityOAuthRequest,
	CapabilityOAuthTokenStatus,
	CapabilityRegistrySnapshot,
	MarketplaceApprovalDecision,
	MarketplaceBrowseQuery,
	MarketplaceEnableRequest,
	MarketplaceRemoveRequest,
	MarketplaceScope,
	MarketplaceSyncRequest,
	NpxSkillsDiscoveryRequest,
	NpxSkillsDiscoveryResult,
	NpxSkillsImportRequest,
	RoutineDetail,
	RoutineDriftOverwriteDecision,
	RoutineDriftOverwriteRequest,
	RoutineDriftResolutionRequest,
	RoutineInstallPreview,
	RoutineInstallPreviewRequest,
	RoutineInstallRequest,
	RoutineInvocationMetadata,
	RoutineInvocationRequest,
	RoutineRegistrySnapshot,
	RoutineRollbackRequest,
} from "../marketplace";

import { Schema } from "effect";

import { NegotiatedBackendTraceMetadata, NegotiatedFrontendTraceMetadata } from "./trace";

/** Requests progressive routine discovery without disclosing routine instructions. */
export const RoutineRegistryQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.list.query"),
	payload: MarketplaceBrowseQuery,
});
export type RoutineRegistryQueryEnvelope = typeof RoutineRegistryQueryEnvelope.Type;
export const RoutineRegistryQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.routine.list.query.result"),
	payload: RoutineRegistrySnapshot,
});
export type RoutineRegistryQueryResultEnvelope = typeof RoutineRegistryQueryResultEnvelope.Type;
export const RoutineDetailQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.detail.query"),
	payload: Schema.Struct({ routine_id: Identifier, scope: MarketplaceScope }),
});
export type RoutineDetailQueryEnvelope = typeof RoutineDetailQueryEnvelope.Type;
export const RoutineDetailQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.routine.detail.query.result"),
	payload: RoutineDetail,
});
export type RoutineDetailQueryResultEnvelope = typeof RoutineDetailQueryResultEnvelope.Type;
export const RoutineInstallPreviewEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.install.preview"),
	payload: RoutineInstallPreviewRequest,
});
export type RoutineInstallPreviewEnvelope = typeof RoutineInstallPreviewEnvelope.Type;
export const RoutineInstallPreviewResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.routine.install.preview.result"),
	payload: RoutineInstallPreview,
});
export type RoutineInstallPreviewResultEnvelope = typeof RoutineInstallPreviewResultEnvelope.Type;
/** Requests only an approval-bound install; it never performs installation by decoding this frame. */
export const RoutineInstallRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.install.request"),
	payload: RoutineInstallRequest,
});
export type RoutineInstallRequestEnvelope = typeof RoutineInstallRequestEnvelope.Type;
export const RoutineApprovalDecisionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.install.decision"),
	payload: MarketplaceApprovalDecision,
});
export type RoutineApprovalDecisionEnvelope = typeof RoutineApprovalDecisionEnvelope.Type;
export const RoutineEnableEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.enable"),
	payload: MarketplaceEnableRequest,
});
export type RoutineEnableEnvelope = typeof RoutineEnableEnvelope.Type;
export const RoutineDisableEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.disable"),
	payload: MarketplaceEnableRequest,
});
export type RoutineDisableEnvelope = typeof RoutineDisableEnvelope.Type;
export const RoutineRemoveEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.remove"),
	payload: MarketplaceRemoveRequest,
});
export type RoutineRemoveEnvelope = typeof RoutineRemoveEnvelope.Type;
export const RoutineSyncEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.sync"),
	payload: MarketplaceSyncRequest,
});
export type RoutineSyncEnvelope = typeof RoutineSyncEnvelope.Type;
export const RoutineDriftResolutionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.drift.resolve"),
	payload: RoutineDriftResolutionRequest,
});
export type RoutineDriftResolutionEnvelope = typeof RoutineDriftResolutionEnvelope.Type;
export const RoutineDriftOverwriteRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.drift.overwrite.request"),
	payload: RoutineDriftOverwriteRequest,
});
export type RoutineDriftOverwriteRequestEnvelope = typeof RoutineDriftOverwriteRequestEnvelope.Type;
export const RoutineDriftOverwriteDecisionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.drift.overwrite.decision"),
	payload: RoutineDriftOverwriteDecision,
});
export type RoutineDriftOverwriteDecisionEnvelope =
	typeof RoutineDriftOverwriteDecisionEnvelope.Type;
export const RoutineInvokeEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.invoke"),
	payload: RoutineInvocationRequest,
});
export type RoutineInvokeEnvelope = typeof RoutineInvokeEnvelope.Type;
export const RoutineInvokeResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.routine.invoke.result"),
	payload: RoutineInvocationMetadata,
});
export type RoutineInvokeResultEnvelope = typeof RoutineInvokeResultEnvelope.Type;
export const RoutineRollbackEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.rollback"),
	payload: RoutineRollbackRequest,
});
export type RoutineRollbackEnvelope = typeof RoutineRollbackEnvelope.Type;
export const NpxSkillsDiscoverEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.npx_skills.discover"),
	payload: NpxSkillsDiscoveryRequest,
});
export type NpxSkillsDiscoverEnvelope = typeof NpxSkillsDiscoverEnvelope.Type;
export const NpxSkillsDiscoverResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.npx_skills.discover.result"),
	payload: NpxSkillsDiscoveryResult,
});
export type NpxSkillsDiscoverResultEnvelope = typeof NpxSkillsDiscoverResultEnvelope.Type;
export const NpxSkillsImportEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.npx_skills.import.request"),
	payload: NpxSkillsImportRequest,
});
export type NpxSkillsImportEnvelope = typeof NpxSkillsImportEnvelope.Type;

export const CapabilityRegistryQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.list.query"),
	payload: MarketplaceBrowseQuery,
});
export type CapabilityRegistryQueryEnvelope = typeof CapabilityRegistryQueryEnvelope.Type;
export const CapabilityRegistryQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.capability.list.query.result"),
	payload: CapabilityRegistrySnapshot,
});
export type CapabilityRegistryQueryResultEnvelope =
	typeof CapabilityRegistryQueryResultEnvelope.Type;
export const CapabilityDetailQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.detail.query"),
	payload: Schema.Struct({ capability_id: Identifier, scope: MarketplaceScope }),
});
export type CapabilityDetailQueryEnvelope = typeof CapabilityDetailQueryEnvelope.Type;
export const CapabilityDetailQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.capability.detail.query.result"),
	payload: CapabilityDetail,
});
export type CapabilityDetailQueryResultEnvelope = typeof CapabilityDetailQueryResultEnvelope.Type;
export const CapabilityConnectPreviewEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.connect.preview"),
	payload: CapabilityConnectPreviewRequest,
});
export type CapabilityConnectPreviewEnvelope = typeof CapabilityConnectPreviewEnvelope.Type;
export const CapabilityConnectPreviewResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.capability.connect.preview.result"),
	payload: CapabilityConnectPreview,
});
export type CapabilityConnectPreviewResultEnvelope =
	typeof CapabilityConnectPreviewResultEnvelope.Type;
/** Requests an approval-bound connect; start/connect happens only after a separate decision. */
export const CapabilityConnectRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.connect.request"),
	payload: CapabilityConnectRequest,
});
export type CapabilityConnectRequestEnvelope = typeof CapabilityConnectRequestEnvelope.Type;
export const CapabilityApprovalDecisionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.connect.decision"),
	payload: MarketplaceApprovalDecision,
});
export type CapabilityApprovalDecisionEnvelope = typeof CapabilityApprovalDecisionEnvelope.Type;

const CapabilityActionEnvelope = <const Kind extends string>(kind: Kind) =>
	Schema.Struct({
		...NegotiatedFrontendTraceMetadata,
		kind: Schema.Literal(kind),
		payload: CapabilityLifecycleRequest,
	});
export const CapabilityStartEnvelope = CapabilityActionEnvelope("marketplace.capability.start");
export const CapabilityReconnectEnvelope = CapabilityActionEnvelope(
	"marketplace.capability.reconnect",
);
export const CapabilityDisconnectEnvelope = CapabilityActionEnvelope(
	"marketplace.capability.disconnect",
);
export const CapabilityRestartEnvelope = CapabilityActionEnvelope("marketplace.capability.restart");
export const CapabilityUninstallEnvelope = CapabilityActionEnvelope(
	"marketplace.capability.uninstall",
);
export type CapabilityStartEnvelope = typeof CapabilityStartEnvelope.Type;
export type CapabilityReconnectEnvelope = typeof CapabilityReconnectEnvelope.Type;
export type CapabilityDisconnectEnvelope = typeof CapabilityDisconnectEnvelope.Type;
export type CapabilityRestartEnvelope = typeof CapabilityRestartEnvelope.Type;
export type CapabilityUninstallEnvelope = typeof CapabilityUninstallEnvelope.Type;
export const CapabilityHealthEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.health"),
	payload: CapabilityHealthRequest,
});
export type CapabilityHealthEnvelope = typeof CapabilityHealthEnvelope.Type;
export const CapabilityEnableEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.enable"),
	payload: MarketplaceEnableRequest,
});
export const CapabilityDisableEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.disable"),
	payload: MarketplaceEnableRequest,
});
export const CapabilityRemoveEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.remove"),
	payload: MarketplaceRemoveRequest,
});
export const CapabilitySyncEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.sync"),
	payload: MarketplaceSyncRequest,
});
export const CapabilityDriftResolutionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.drift.resolve"),
	payload: CapabilityDriftResolutionRequest,
});
export const CapabilityInvokeEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.invoke"),
	payload: CapabilityInvocationRequest,
});
export type CapabilityEnableEnvelope = typeof CapabilityEnableEnvelope.Type;
export type CapabilityDisableEnvelope = typeof CapabilityDisableEnvelope.Type;
export type CapabilityRemoveEnvelope = typeof CapabilityRemoveEnvelope.Type;
export type CapabilitySyncEnvelope = typeof CapabilitySyncEnvelope.Type;
export type CapabilityDriftResolutionEnvelope = typeof CapabilityDriftResolutionEnvelope.Type;
export const CapabilityDriftOverwriteRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.drift.overwrite.request"),
	payload: CapabilityDriftOverwriteRequest,
});
export type CapabilityDriftOverwriteRequestEnvelope =
	typeof CapabilityDriftOverwriteRequestEnvelope.Type;
export const CapabilityDriftOverwriteDecisionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.drift.overwrite.decision"),
	payload: CapabilityDriftOverwriteDecision,
});
export type CapabilityDriftOverwriteDecisionEnvelope =
	typeof CapabilityDriftOverwriteDecisionEnvelope.Type;
export const CapabilityInvocationApprovalRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.invoke.request"),
	payload: CapabilityInvocationApprovalRequest,
});
export type CapabilityInvocationApprovalRequestEnvelope =
	typeof CapabilityInvocationApprovalRequestEnvelope.Type;
export const CapabilityInvocationApprovalDecisionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.invoke.decision"),
	payload: CapabilityInvocationApprovalDecision,
});
export type CapabilityInvocationApprovalDecisionEnvelope =
	typeof CapabilityInvocationApprovalDecisionEnvelope.Type;
export type CapabilityInvokeEnvelope = typeof CapabilityInvokeEnvelope.Type;
export const CapabilityInvokeResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.capability.invoke.result"),
	payload: CapabilityInvocationMetadata,
});
export type CapabilityInvokeResultEnvelope = typeof CapabilityInvokeResultEnvelope.Type;
export const CapabilityOAuthBeginEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.oauth.begin"),
	payload: CapabilityOAuthRequest,
});
export const CapabilityOAuthBeginResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.capability.oauth.begin.result"),
	payload: CapabilityOAuthBeginResult,
});
export const CapabilityOAuthCompleteEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.oauth.complete"),
	payload: CapabilityOAuthCompleteRequest,
});
export const CapabilityOAuthRefreshEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.oauth.refresh"),
	payload: CapabilityOAuthRequest,
});
export const CapabilityOAuthRevokeEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.oauth.revoke"),
	payload: CapabilityOAuthRequest,
});
export const CapabilityOAuthTokenStatusEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.oauth.status.query"),
	payload: CapabilityOAuthRequest,
});
export const CapabilityOAuthTokenStatusResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.capability.oauth.status.query.result"),
	payload: CapabilityOAuthTokenStatus,
});
export type CapabilityOAuthBeginEnvelope = typeof CapabilityOAuthBeginEnvelope.Type;
export type CapabilityOAuthBeginResultEnvelope = typeof CapabilityOAuthBeginResultEnvelope.Type;
export type CapabilityOAuthCompleteEnvelope = typeof CapabilityOAuthCompleteEnvelope.Type;
export type CapabilityOAuthRefreshEnvelope = typeof CapabilityOAuthRefreshEnvelope.Type;
export type CapabilityOAuthRevokeEnvelope = typeof CapabilityOAuthRevokeEnvelope.Type;
export type CapabilityOAuthTokenStatusEnvelope = typeof CapabilityOAuthTokenStatusEnvelope.Type;
export type CapabilityOAuthTokenStatusResultEnvelope =
	typeof CapabilityOAuthTokenStatusResultEnvelope.Type;
