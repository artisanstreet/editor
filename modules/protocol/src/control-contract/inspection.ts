import { MessageImageAttachmentQuery, MessageImageAttachmentQueryResult } from "../attachments";

import { Identifier } from "../common";

import { ConversationQuery, ConversationSnapshot } from "../conversation";
import { ThreadListItem } from "../thread";

import { ThreadUsageSeries, ThreadUsageSeriesQuery } from "../thread-usage-series";
import { EngineUsageQuery, EngineUsageSnapshot } from "../engine-usage";

import { HostIdentitySnapshot } from "../host-identity";

import {
	OrchestrationGroupListQuery,
	OrchestrationGroupListSnapshot,
} from "../orchestration-groups";

import {
	PreviewAssetMetadataQuery,
	PreviewBrowserLaunch,
	PreviewBrowserLaunchRequest,
	PreviewInspectionRequest,
	PreviewInspectionResult,
	PreviewInspectionSession,
	PreviewInspectionSessionCloseRequest,
	PreviewInspectionSessionOpenRequest,
	PreviewTarget,
	PreviewTargetGetQuery,
	PreviewTargetListQuery,
	PreviewTargetRegistration,
	PreviewTargetRemoveRequest,
	PreviewTargetStateRequest,
	RichLinkAssetMetadata,
	RichLinkResolution,
	RichLinkResolveQuery,
} from "../preview";

import {
	ProjectDiffQuery,
	ProjectDiffQueryResult,
	ProjectRepositoryQuery,
	ProjectRepositoryQueryResult,
} from "../repository";

import {
	SurfaceListQuery,
	SurfaceSnapshot,
	SurfaceUsageAggregateQuery,
	SurfaceUsageAggregateSnapshot,
	SurfaceUsageDailyQuery,
	SurfaceUsageDailySnapshot,
} from "../surfaces";

import { ThreadTranscriptQuery, ThreadTranscriptSnapshot } from "../transcript";

import { OrchestrationGraph, TerminalSession, ThreadSessionSnapshot } from "./lifecycle";

import { Schema } from "effect";

import { NegotiatedBackendTraceMetadata, NegotiatedFrontendTraceMetadata } from "./trace";

/** Describes the durable work state coordinated for one thread. */
export const ThreadWorkItem = Schema.Struct({
	agent_id: Identifier,
	display_name: Schema.NonEmptyString,
	engine_id: Identifier,
	native_thread_id: Schema.optional(Identifier),
	role: Schema.NonEmptyString,
	run_id: Identifier,
	status: Schema.Literals([
		"queued",
		"running",
		"waiting",
		"interrupted",
		"completed",
		"cancelled",
		"failed",
		"closed",
	]),
});

export type ThreadWorkItem = typeof ThreadWorkItem.Type;

/** Requests the current durable coordinator work for one thread. */
export const ThreadWorkQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("thread.work.query"),
	payload: Schema.Struct({ thread_id: Identifier }),
});

export type ThreadWorkQueryEnvelope = typeof ThreadWorkQueryEnvelope.Type;

/** Returns the durable coordinator work for one thread. */
export const ThreadWorkQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("thread.work.query.result"),
	payload: Schema.Struct({ work: Schema.optional(ThreadWorkItem) }),
});

export type ThreadWorkQueryResultEnvelope = typeof ThreadWorkQueryResultEnvelope.Type;

/** The authoritative, single-round-trip state required to open a thread. */
export const ThreadOpenSnapshot = Schema.Struct({
	conversation: ConversationSnapshot,
	session: ThreadSessionSnapshot,
	thread: ThreadListItem,
	work: Schema.optional(ThreadWorkItem),
});
export type ThreadOpenSnapshot = typeof ThreadOpenSnapshot.Type;

export const ThreadOpenQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("thread.open.query"),
	payload: Schema.Struct({ thread_id: Identifier }),
});
export type ThreadOpenQueryEnvelope = typeof ThreadOpenQueryEnvelope.Type;

export const ThreadOpenQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("thread.open.query.result"),
	payload: ThreadOpenSnapshot,
});
export type ThreadOpenQueryResultEnvelope = typeof ThreadOpenQueryResultEnvelope.Type;

/** Requests terminal metadata for one thread workspace. */
export const TerminalListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("terminal.list.query"),
	payload: Schema.Struct({ thread_id: Identifier, workspace_id: Identifier }),
});

export type TerminalListQueryEnvelope = typeof TerminalListQueryEnvelope.Type;

/** Returns durable terminal metadata without replaying transient PTY output. */
export const TerminalListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("terminal.list.query.result"),
	payload: Schema.Struct({ terminals: Schema.Array(TerminalSession) }),
});

export type TerminalListQueryResultEnvelope = typeof TerminalListQueryResultEnvelope.Type;

/** Requests the complete durable projection for one orchestration group. */
export const OrchestrationGraphQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("orchestration.graph.query"),
	payload: Schema.Struct({ group_id: Identifier }),
});

export type OrchestrationGraphQueryEnvelope = typeof OrchestrationGraphQueryEnvelope.Type;

/** Lists explicit local preview targets without rendering their pages in Artisan. */
export const PreviewTargetListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.target.list.query"),
	payload: PreviewTargetListQuery,
});
export type PreviewTargetListQueryEnvelope = typeof PreviewTargetListQueryEnvelope.Type;

export const PreviewTargetListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.target.list.query.result"),
	payload: Schema.Struct({ targets: Schema.Array(PreviewTarget) }),
});
export type PreviewTargetListQueryResultEnvelope = typeof PreviewTargetListQueryResultEnvelope.Type;

export const PreviewTargetGetQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.target.get.query"),
	payload: PreviewTargetGetQuery,
});
export type PreviewTargetGetQueryEnvelope = typeof PreviewTargetGetQueryEnvelope.Type;

export const PreviewTargetGetQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.target.get.query.result"),
	payload: PreviewTarget,
});
export type PreviewTargetGetQueryResultEnvelope = typeof PreviewTargetGetQueryResultEnvelope.Type;

export const PreviewTargetRegisterEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.target.register"),
	payload: PreviewTargetRegistration,
});
export type PreviewTargetRegisterEnvelope = typeof PreviewTargetRegisterEnvelope.Type;

export const PreviewTargetProbeEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.target.probe"),
	payload: PreviewTargetGetQuery,
});
export type PreviewTargetProbeEnvelope = typeof PreviewTargetProbeEnvelope.Type;

export const PreviewTargetStateEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.target.state"),
	payload: PreviewTargetStateRequest,
});
export type PreviewTargetStateEnvelope = typeof PreviewTargetStateEnvelope.Type;

export const PreviewTargetRemoveEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.target.remove"),
	payload: PreviewTargetRemoveRequest,
});
export type PreviewTargetRemoveEnvelope = typeof PreviewTargetRemoveEnvelope.Type;

/** Returns a correlated target mutation result; removal returns the removed target for lifecycle attribution. */
export const PreviewTargetMutationResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.target.mutation.result"),
	payload: PreviewTarget,
});
export type PreviewTargetMutationResultEnvelope = typeof PreviewTargetMutationResultEnvelope.Type;

export const RichLinkResolveQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.rich_link.resolve.query"),
	payload: RichLinkResolveQuery,
});
export type RichLinkResolveQueryEnvelope = typeof RichLinkResolveQueryEnvelope.Type;

export const RichLinkResolveQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.rich_link.resolve.query.result"),
	payload: RichLinkResolution,
});
export type RichLinkResolveQueryResultEnvelope = typeof RichLinkResolveQueryResultEnvelope.Type;

export const PreviewAssetMetadataQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.asset.metadata.query"),
	payload: PreviewAssetMetadataQuery,
});
export type PreviewAssetMetadataQueryEnvelope = typeof PreviewAssetMetadataQueryEnvelope.Type;

export const PreviewAssetMetadataQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.asset.metadata.query.result"),
	payload: RichLinkAssetMetadata,
});
export type PreviewAssetMetadataQueryResultEnvelope =
	typeof PreviewAssetMetadataQueryResultEnvelope.Type;

export const PreviewBrowserLaunchEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.browser.launch"),
	payload: PreviewBrowserLaunchRequest,
});
export type PreviewBrowserLaunchEnvelope = typeof PreviewBrowserLaunchEnvelope.Type;

export const PreviewBrowserLaunchResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.browser.launch.result"),
	payload: PreviewBrowserLaunch,
});
export type PreviewBrowserLaunchResultEnvelope = typeof PreviewBrowserLaunchResultEnvelope.Type;

export const PreviewInspectionSessionOpenEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.inspection.open"),
	payload: PreviewInspectionSessionOpenRequest,
});
export type PreviewInspectionSessionOpenEnvelope = typeof PreviewInspectionSessionOpenEnvelope.Type;

export const PreviewInspectionSessionOpenResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.inspection.open.result"),
	payload: PreviewInspectionSession,
});
export type PreviewInspectionSessionOpenResultEnvelope =
	typeof PreviewInspectionSessionOpenResultEnvelope.Type;

export const PreviewInspectionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.inspection.inspect"),
	payload: PreviewInspectionRequest,
});
export type PreviewInspectionEnvelope = typeof PreviewInspectionEnvelope.Type;

export const PreviewInspectionResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.inspection.inspect.result"),
	payload: PreviewInspectionResult,
});
export type PreviewInspectionResultEnvelope = typeof PreviewInspectionResultEnvelope.Type;

export const PreviewInspectionSessionCloseEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.inspection.close"),
	payload: PreviewInspectionSessionCloseRequest,
});
export type PreviewInspectionSessionCloseEnvelope =
	typeof PreviewInspectionSessionCloseEnvelope.Type;

export const PreviewInspectionSessionCloseResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.inspection.close.result"),
	payload: PreviewInspectionSession,
});
export type PreviewInspectionSessionCloseResultEnvelope =
	typeof PreviewInspectionSessionCloseResultEnvelope.Type;

/** Returns one provider-neutral orchestration graph projection. */
export const OrchestrationGraphQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("orchestration.graph.query.result"),
	payload: Schema.Struct({ graph: OrchestrationGraph }),
});

export type OrchestrationGraphQueryResultEnvelope =
	typeof OrchestrationGraphQueryResultEnvelope.Type;

/** Requests renderer-safe, bounded journal facts for one thread. */
export const ThreadTranscriptQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("thread.transcript.query"),
	payload: ThreadTranscriptQuery,
});
export type ThreadTranscriptQueryEnvelope = typeof ThreadTranscriptQueryEnvelope.Type;

export const ThreadTranscriptQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("thread.transcript.query.result"),
	payload: ThreadTranscriptSnapshot,
});
export type ThreadTranscriptQueryResultEnvelope = typeof ThreadTranscriptQueryResultEnvelope.Type;

/** Requests the canonical renderer-ready conversation projection for one thread. */
export const ConversationQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("conversation.query"),
	payload: ConversationQuery,
});
export type ConversationQueryEnvelope = typeof ConversationQueryEnvelope.Type;

export const ConversationQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("conversation.query.result"),
	payload: ConversationSnapshot,
});
export type ConversationQueryResultEnvelope = typeof ConversationQueryResultEnvelope.Type;

/** Reads one persisted user image without widening conversation snapshots or events. */
export const MessageImageAttachmentQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("message.image_attachment.query"),
	payload: MessageImageAttachmentQuery,
});
export type MessageImageAttachmentQueryEnvelope = typeof MessageImageAttachmentQueryEnvelope.Type;

export const MessageImageAttachmentQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("message.image_attachment.query.result"),
	payload: MessageImageAttachmentQueryResult,
});
export type MessageImageAttachmentQueryResultEnvelope =
	typeof MessageImageAttachmentQueryResultEnvelope.Type;

/** Discovers a thread's current and historic orchestration groups without a known id. */
export const OrchestrationGroupListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("orchestration.group.list.query"),
	payload: OrchestrationGroupListQuery,
});
export type OrchestrationGroupListQueryEnvelope = typeof OrchestrationGroupListQueryEnvelope.Type;

export const OrchestrationGroupListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("orchestration.group.list.query.result"),
	payload: OrchestrationGroupListSnapshot,
});
export type OrchestrationGroupListQueryResultEnvelope =
	typeof OrchestrationGroupListQueryResultEnvelope.Type;

export const ThreadSessionQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("thread.session.query"),
	payload: Schema.Struct({ thread_id: Identifier }),
});
export type ThreadSessionQueryEnvelope = typeof ThreadSessionQueryEnvelope.Type;
export const ThreadSessionQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("thread.session.query.result"),
	payload: ThreadSessionSnapshot,
});
export type ThreadSessionQueryResultEnvelope = typeof ThreadSessionQueryResultEnvelope.Type;

export const SurfaceListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("surface.list.query"),
	payload: SurfaceListQuery,
});
export type SurfaceListQueryEnvelope = typeof SurfaceListQueryEnvelope.Type;
export const SurfaceListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("surface.list.query.result"),
	payload: SurfaceSnapshot,
});
export type SurfaceListQueryResultEnvelope = typeof SurfaceListQueryResultEnvelope.Type;

export const SurfaceUsageAggregateQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("surface.usage.aggregate.query"),
	payload: SurfaceUsageAggregateQuery,
});
export type SurfaceUsageAggregateQueryEnvelope = typeof SurfaceUsageAggregateQueryEnvelope.Type;
export const SurfaceUsageAggregateQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("surface.usage.aggregate.query.result"),
	payload: SurfaceUsageAggregateSnapshot,
});
export type SurfaceUsageAggregateQueryResultEnvelope =
	typeof SurfaceUsageAggregateQueryResultEnvelope.Type;

export const SurfaceUsageDailyQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("surface.usage.daily.query"),
	payload: SurfaceUsageDailyQuery,
});
export type SurfaceUsageDailyQueryEnvelope = typeof SurfaceUsageDailyQueryEnvelope.Type;
export const SurfaceUsageDailyQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("surface.usage.daily.query.result"),
	payload: SurfaceUsageDailySnapshot,
});
export type SurfaceUsageDailyQueryResultEnvelope = typeof SurfaceUsageDailyQueryResultEnvelope.Type;

/** Requests the signed-in OS account identity of the machine hosting Forge. */
export const HostIdentityQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("host.identity.query"),
	payload: Schema.Struct({}),
});
export type HostIdentityQueryEnvelope = typeof HostIdentityQueryEnvelope.Type;
export const HostIdentityQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("host.identity.query.result"),
	payload: HostIdentitySnapshot,
});
export type HostIdentityQueryResultEnvelope = typeof HostIdentityQueryResultEnvelope.Type;

/** Requests repository identity for projects Forge already owns. */
export const ProjectRepositoryQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("project.repository.query"),
	payload: ProjectRepositoryQuery,
});
export type ProjectRepositoryQueryEnvelope = typeof ProjectRepositoryQueryEnvelope.Type;
export const ProjectRepositoryQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("project.repository.query.result"),
	payload: ProjectRepositoryQueryResult,
});
export type ProjectRepositoryQueryResultEnvelope = typeof ProjectRepositoryQueryResultEnvelope.Type;

/** Requests uncommitted diff summaries for projects Forge already owns. */
export const ProjectDiffQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("project.diff.query"),
	payload: ProjectDiffQuery,
});
export type ProjectDiffQueryEnvelope = typeof ProjectDiffQueryEnvelope.Type;
export const ProjectDiffQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("project.diff.query.result"),
	payload: ProjectDiffQueryResult,
});
export type ProjectDiffQueryResultEnvelope = typeof ProjectDiffQueryResultEnvelope.Type;

/** Requests provider-account quota usage for every registered engine. */
export const EngineUsageQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("engine.usage.query"),
	payload: EngineUsageQuery,
});
export type EngineUsageQueryEnvelope = typeof EngineUsageQueryEnvelope.Type;
export const EngineUsageQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("engine.usage.query.result"),
	payload: EngineUsageSnapshot,
});
export type EngineUsageQueryResultEnvelope = typeof EngineUsageQueryResultEnvelope.Type;

/** Requests the per-turn token series for one thread's current context window. */
export const ThreadUsageSeriesQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("thread.usage.series.query"),
	payload: ThreadUsageSeriesQuery,
});
export type ThreadUsageSeriesQueryEnvelope = typeof ThreadUsageSeriesQueryEnvelope.Type;
export const ThreadUsageSeriesQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("thread.usage.series.query.result"),
	payload: ThreadUsageSeries,
});
export type ThreadUsageSeriesQueryResultEnvelope = typeof ThreadUsageSeriesQueryResultEnvelope.Type;
