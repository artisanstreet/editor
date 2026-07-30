import {
	ArtisanApprovalListQuery,
	ArtisanApprovalListQueryResult,
	ArtisanApprovalResolveRequest,
	ArtisanToolExecutionRequest,
	ArtisanToolInvocationListQuery,
	ArtisanToolInvocationListQueryResult,
	ArtisanToolRegistryListQuery,
	ArtisanToolRegistryListQueryResult,
	WorkspaceFileDiscoveryQuery,
	WorkspaceFileDiscoveryQueryResult,
	WorkspaceLanguageCapabilitiesQuery,
	WorkspaceLanguageCapabilitiesQueryResult,
} from "../artisan-tools";

import { Identifier, RawOrigin } from "../common";

import { Schema } from "effect";

import { NegotiatedBackendTraceMetadata, NegotiatedFrontendTraceMetadata } from "./trace";

/** Requests the policy-aware built-in tool registry and renderer-safe usage snapshot. */
export const ArtisanToolRegistryListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("artisan.tool.registry.list.query"),
	payload: ArtisanToolRegistryListQuery,
});

export type ArtisanToolRegistryListQueryEnvelope = typeof ArtisanToolRegistryListQueryEnvelope.Type;

/** Returns available built-in tools, their policy state, and bounded usage totals. */
export const ArtisanToolRegistryListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("artisan.tool.registry.list.query.result"),
	payload: ArtisanToolRegistryListQueryResult,
});

export type ArtisanToolRegistryListQueryResultEnvelope =
	typeof ArtisanToolRegistryListQueryResultEnvelope.Type;

/** Starts one durable policy-routed Artisan-owned tool invocation. */
export const ArtisanToolExecuteEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	agent_id: Schema.optional(Identifier),
	kind: Schema.Literal("artisan.tool.execute"),
	payload: ArtisanToolExecutionRequest,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
});

export type ArtisanToolExecuteEnvelope = typeof ArtisanToolExecuteEnvelope.Type;

/** Resolves one exact pending tool approval through the canonical policy path. */
export const ArtisanApprovalResolveEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	agent_id: Schema.optional(Identifier),
	kind: Schema.Literal("artisan.approval.resolve"),
	payload: ArtisanApprovalResolveRequest,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
});

export type ArtisanApprovalResolveEnvelope = typeof ArtisanApprovalResolveEnvelope.Type;

/** Requests bounded, cursor-aware invocation history for one visible thread. */
export const ArtisanToolInvocationListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("artisan.tool.invocation.list.query"),
	payload: ArtisanToolInvocationListQuery,
});

export type ArtisanToolInvocationListQueryEnvelope =
	typeof ArtisanToolInvocationListQueryEnvelope.Type;

/** Returns invocation history and its durable ledger position. */
export const ArtisanToolInvocationListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("artisan.tool.invocation.list.query.result"),
	payload: ArtisanToolInvocationListQueryResult,
});

export type ArtisanToolInvocationListQueryResultEnvelope =
	typeof ArtisanToolInvocationListQueryResultEnvelope.Type;

/** Requests pending or resolved approvals belonging to one visible thread. */
export const ArtisanApprovalListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("artisan.approval.list.query"),
	payload: ArtisanApprovalListQuery,
});

export type ArtisanApprovalListQueryEnvelope = typeof ArtisanApprovalListQueryEnvelope.Type;

/** Returns durable approvals and the ledger position represented by the response. */
export const ArtisanApprovalListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("artisan.approval.list.query.result"),
	payload: ArtisanApprovalListQueryResult,
});

export type ArtisanApprovalListQueryResultEnvelope =
	typeof ArtisanApprovalListQueryResultEnvelope.Type;

/** Requests root-confined workspace path metadata without exposing filesystem access to renderers. */
export const WorkspaceFileDiscoveryQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.file.discovery.query"),
	payload: WorkspaceFileDiscoveryQuery,
});

export type WorkspaceFileDiscoveryQueryEnvelope = typeof WorkspaceFileDiscoveryQueryEnvelope.Type;

/** Returns one bounded page of content-free workspace path metadata. */
export const WorkspaceFileDiscoveryQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("workspace.file.discovery.query.result"),
	payload: WorkspaceFileDiscoveryQueryResult,
});

export type WorkspaceFileDiscoveryQueryResultEnvelope =
	typeof WorkspaceFileDiscoveryQueryResultEnvelope.Type;

/** Requests the truthful backend-owned language and diagnostics capability projection. */
export const WorkspaceLanguageCapabilitiesQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.language.capabilities.query"),
	payload: WorkspaceLanguageCapabilitiesQuery,
});

export type WorkspaceLanguageCapabilitiesQueryEnvelope =
	typeof WorkspaceLanguageCapabilitiesQueryEnvelope.Type;

/** Returns language feature availability without implying local renderer capabilities. */
export const WorkspaceLanguageCapabilitiesQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("workspace.language.capabilities.query.result"),
	payload: WorkspaceLanguageCapabilitiesQueryResult,
});

export type WorkspaceLanguageCapabilitiesQueryResultEnvelope =
	typeof WorkspaceLanguageCapabilitiesQueryResultEnvelope.Type;
