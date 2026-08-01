import { Identifier, JournalSequence, RawOrigin } from "../common";

import {
	GitDiffQuery,
	GitDiffQueryResult,
	GitIndexStageRequest,
	GitIndexUnstageRequest,
	GitMutationResolveRequest,
	GitWorkspaceQuery,
	GitWorkspaceQueryResult,
} from "../git";

import {
	GlobalGuidanceDriftResolutionRequest,
	GlobalGuidanceRetryRequest,
	GlobalGuidanceSelectionRequest,
	GlobalGuidanceSnapshot,
	GlobalGuidanceUpdateRequest,
} from "../guidance";

import {
	ModelBehaviourDriftResolutionRequest,
	ModelBehaviourRetryRequest,
	ModelBehaviourSnapshot,
	ModelBehaviourUpdateRequest,
} from "../model-behaviour";

import { ModelFavoritesSnapshot } from "../model-favorites";

import { Project, ProjectCatalogSnapshot, ProjectDetachInput } from "../project";

import {
	ProjectDirectoryCreateInput,
	ProjectDirectoryEntry,
	ProjectDirectoryList,
	ProjectDirectoryListInput,
	ProjectDirectorySelectInput,
} from "../project-directory";

import { RuntimeCatalog } from "../runtime-catalog";

import { SessionDefaults, SessionDefaultsUpdateInput } from "../session-defaults";

import { ThreadCreateInput, ThreadListItem, ThreadRetentionPolicy } from "../thread";

import {
	WorkspaceChangeDiffQuery,
	WorkspaceChangeDiffQueryResult,
	WorkspaceChangeListQuery,
	WorkspaceChangeListQueryResult,
	WorkspaceChangeReviewRequest,
	WorkspaceChangeRollbackRequest,
	WorkspaceConflictListQuery,
	WorkspaceConflictListQueryResult,
	WorkspaceFileReadQuery,
	WorkspaceFileReadQueryResult,
	WorkspaceFileReplaceRequest,
} from "../workspace-changes";

import { Schema } from "effect";

import { NegotiatedBackendTraceMetadata, NegotiatedFrontendTraceMetadata } from "./trace";

/** Requests the current thread-list projection from the backend. */
export const ThreadListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("thread.list.query"),
	payload: Schema.Struct({}),
});

export type ThreadListQueryEnvelope = typeof ThreadListQueryEnvelope.Type;

/** Returns the current thread-list projection for a correlated query. */
export const ThreadListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("thread.list.query.result"),
	payload: Schema.Struct({
		journal_sequence: JournalSequence,
		threads: Schema.Array(ThreadListItem),
	}),
});

export type ThreadListQueryResultEnvelope = typeof ThreadListQueryResultEnvelope.Type;

/** Requests a new Forge-owned thread without accepting a client-selected identity. */
export const ThreadCreateEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("thread.create.request"),
	payload: ThreadCreateInput,
});

export type ThreadCreateEnvelope = typeof ThreadCreateEnvelope.Type;

/** Returns the complete authoritative projection for the newly created thread. */
export const ThreadCreateResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("thread.create.result"),
	payload: ThreadListItem,
});

export type ThreadCreateResultEnvelope = typeof ThreadCreateResultEnvelope.Type;

/** Lists allowed server-side roots or the children of one opaque directory id. */
export const ProjectDirectoryListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("project.directory.list.query"),
	payload: ProjectDirectoryListInput,
});
export type ProjectDirectoryListQueryEnvelope = typeof ProjectDirectoryListQueryEnvelope.Type;

/** Returns bounded browser-safe directory metadata. */
export const ProjectDirectoryListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("project.directory.list.query.result"),
	payload: ProjectDirectoryList,
});
export type ProjectDirectoryListQueryResultEnvelope =
	typeof ProjectDirectoryListQueryResultEnvelope.Type;

/** Resolves an opaque directory id to a canonical project reference. */
export const ProjectDirectorySelectEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("project.directory.select"),
	payload: ProjectDirectorySelectInput,
});
export type ProjectDirectorySelectEnvelope = typeof ProjectDirectorySelectEnvelope.Type;

/** Returns the canonical project selected by the server-side locator. */
export const ProjectDirectorySelectResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("project.directory.select.result"),
	payload: Project,
});
export type ProjectDirectorySelectResultEnvelope = typeof ProjectDirectorySelectResultEnvelope.Type;

/** Creates a named directory inside an already-listed parent directory. */
export const ProjectDirectoryCreateEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("project.directory.create"),
	payload: ProjectDirectoryCreateInput,
});
export type ProjectDirectoryCreateEnvelope = typeof ProjectDirectoryCreateEnvelope.Type;

/** Returns the created directory as a browsable entry. */
export const ProjectDirectoryCreateResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("project.directory.create.result"),
	payload: ProjectDirectoryEntry,
});
export type ProjectDirectoryCreateResultEnvelope = typeof ProjectDirectoryCreateResultEnvelope.Type;

/** Requests the complete authoritative project catalog owned by Forge. */
export const ProjectListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("project.list.query"),
	payload: Schema.Struct({}),
});
export type ProjectListQueryEnvelope = typeof ProjectListQueryEnvelope.Type;

/** Returns the complete authoritative Forge project catalog. */
export const ProjectListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("project.list.query.result"),
	payload: ProjectCatalogSnapshot,
});
export type ProjectListQueryResultEnvelope = typeof ProjectListQueryResultEnvelope.Type;

/** Detaches one Forge-owned project without accepting client filesystem data. */
export const ProjectDetachEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("project.detach"),
	payload: ProjectDetachInput,
});
export type ProjectDetachEnvelope = typeof ProjectDetachEnvelope.Type;

/** Returns the authoritative catalog after a project mutation. */
export const ProjectDetachResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("project.detach.result"),
	payload: ProjectCatalogSnapshot,
});
export type ProjectDetachResultEnvelope = typeof ProjectDetachResultEnvelope.Type;

/** Requests the immutable capability catalog exposed by this Forge process. */
export const RuntimeCatalogQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("runtime.catalog.query"),
	payload: Schema.Struct({}),
});
export type RuntimeCatalogQueryEnvelope = typeof RuntimeCatalogQueryEnvelope.Type;

/** Returns only model and harness capabilities backed by registered Forge adapters. */
export const RuntimeCatalogQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("runtime.catalog.query.result"),
	payload: RuntimeCatalog,
});
export type RuntimeCatalogQueryResultEnvelope = typeof RuntimeCatalogQueryResultEnvelope.Type;

/** Requests the current global inactive-thread retention policy. */
export const ThreadRetentionQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("thread.retention.query"),
	payload: Schema.Struct({}),
});

export type ThreadRetentionQueryEnvelope = typeof ThreadRetentionQueryEnvelope.Type;

/** Returns the current global inactive-thread retention policy. */
export const ThreadRetentionQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("thread.retention.query.result"),
	payload: ThreadRetentionPolicy,
});

export type ThreadRetentionQueryResultEnvelope = typeof ThreadRetentionQueryResultEnvelope.Type;

/** Updates the global inactive-thread retention policy without a synthetic thread id. */
export const ThreadRetentionUpdateEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("thread.retention.update"),
	payload: ThreadRetentionPolicy,
});

export type ThreadRetentionUpdateEnvelope = typeof ThreadRetentionUpdateEnvelope.Type;

/** Requests the defaults a new draft inherits. */
export const SessionDefaultsQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("session.defaults.query"),
	payload: Schema.Struct({}),
});

export type SessionDefaultsQueryEnvelope = typeof SessionDefaultsQueryEnvelope.Type;

/** Returns the defaults a new draft inherits. */
export const SessionDefaultsQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("session.defaults.query.result"),
	payload: SessionDefaults,
});

export type SessionDefaultsQueryResultEnvelope = typeof SessionDefaultsQueryResultEnvelope.Type;

/** Patches the session defaults without a synthetic thread id. */
export const SessionDefaultsUpdateEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("session.defaults.update"),
	payload: SessionDefaultsUpdateInput,
});

export type SessionDefaultsUpdateEnvelope = typeof SessionDefaultsUpdateEnvelope.Type;

/** Requests the complete set of starred catalog models. */
export const ModelFavoritesQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("model.favorites.query"),
	payload: Schema.Struct({}),
});

export type ModelFavoritesQueryEnvelope = typeof ModelFavoritesQueryEnvelope.Type;

/** Returns the complete set of starred catalog models. */
export const ModelFavoritesQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("model.favorites.query.result"),
	payload: ModelFavoritesSnapshot,
});

export type ModelFavoritesQueryResultEnvelope = typeof ModelFavoritesQueryResultEnvelope.Type;

/** Stars or unstars one catalog model without a synthetic thread id. */
export const ModelFavoriteUpdateEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("model.favorite.update"),
	payload: Schema.Struct({
		favorite: Schema.Boolean,
		model_id: Schema.NonEmptyString,
	}),
});

export type ModelFavoriteUpdateEnvelope = typeof ModelFavoriteUpdateEnvelope.Type;

/** Requests the canonical global guidance content and current reconciliation state. */
export const GlobalGuidanceQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("guidance.query"),
	payload: Schema.Struct({}),
});

export type GlobalGuidanceQueryEnvelope = typeof GlobalGuidanceQueryEnvelope.Type;

/** Returns canonical guidance content without ever routing it through the event ledger. */
export const GlobalGuidanceQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("guidance.query.result"),
	payload: GlobalGuidanceSnapshot,
});

export type GlobalGuidanceQueryResultEnvelope = typeof GlobalGuidanceQueryResultEnvelope.Type;

/** Replaces the canonical guidance file through the backend-owned file workflow. */
export const GlobalGuidanceUpdateEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("guidance.update"),
	payload: GlobalGuidanceUpdateRequest,
});

export type GlobalGuidanceUpdateEnvelope = typeof GlobalGuidanceUpdateEnvelope.Type;

/** Selects one freshly rediscovered first-run provider value. */
export const GlobalGuidanceSelectionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("guidance.selection"),
	payload: GlobalGuidanceSelectionRequest,
});

export type GlobalGuidanceSelectionEnvelope = typeof GlobalGuidanceSelectionEnvelope.Type;

/** Resolves one exact provider drift observation. */
export const GlobalGuidanceDriftResolutionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("guidance.drift.resolve"),
	payload: GlobalGuidanceDriftResolutionRequest,
});

export type GlobalGuidanceDriftResolutionEnvelope =
	typeof GlobalGuidanceDriftResolutionEnvelope.Type;

/** Retries one provider's opinionated sync strategy without adding a settings toggle. */
export const GlobalGuidanceRetryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("guidance.sync.retry"),
	payload: GlobalGuidanceRetryRequest,
});

export type GlobalGuidanceRetryEnvelope = typeof GlobalGuidanceRetryEnvelope.Type;

/** Requests the current text and identity for one canonical workspace file. */
export const WorkspaceFileReadQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.file.read.query"),
	payload: WorkspaceFileReadQuery,
});

export type WorkspaceFileReadQueryEnvelope = typeof WorkspaceFileReadQueryEnvelope.Type;

/** Returns the current text and identity for one correlated workspace file query. */
export const WorkspaceFileReadQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("workspace.file.read.query.result"),
	payload: WorkspaceFileReadQueryResult,
});

export type WorkspaceFileReadQueryResultEnvelope = typeof WorkspaceFileReadQueryResultEnvelope.Type;

/** Requests an attributed replacement of one existing UTF-8 regular workspace file. */
export const WorkspaceFileReplaceEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	agent_id: Identifier,
	kind: Schema.Literal("workspace.file.replace"),
	payload: WorkspaceFileReplaceRequest,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Identifier,
	thread_id: Identifier,
});

export type WorkspaceFileReplaceEnvelope = typeof WorkspaceFileReplaceEnvelope.Type;

/** Requests a review transition for one workspace change attributed to a thread. */
export const WorkspaceChangeReviewEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.change.review"),
	payload: WorkspaceChangeReviewRequest,
	thread_id: Identifier,
});

export type WorkspaceChangeReviewEnvelope = typeof WorkspaceChangeReviewEnvelope.Type;

/** Requests a guarded rollback transition for one workspace change attributed to a thread. */
export const WorkspaceChangeRollbackEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.change.rollback"),
	payload: WorkspaceChangeRollbackRequest,
	thread_id: Identifier,
});

export type WorkspaceChangeRollbackEnvelope = typeof WorkspaceChangeRollbackEnvelope.Type;

/** Requests workspace changes attributed to one thread and optionally one workspace. */
export const WorkspaceChangeListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.change.list.query"),
	payload: WorkspaceChangeListQuery,
});

export type WorkspaceChangeListQueryEnvelope = typeof WorkspaceChangeListQueryEnvelope.Type;

/** Returns the durable workspace-change projection for one correlated list query. */
export const WorkspaceChangeListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("workspace.change.list.query.result"),
	payload: WorkspaceChangeListQueryResult,
});

export type WorkspaceChangeListQueryResultEnvelope =
	typeof WorkspaceChangeListQueryResultEnvelope.Type;

export const WorkspaceConflictListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.conflict.list.query"),
	payload: WorkspaceConflictListQuery,
});
export type WorkspaceConflictListQueryEnvelope = typeof WorkspaceConflictListQueryEnvelope.Type;
export const WorkspaceConflictListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("workspace.conflict.list.query.result"),
	payload: WorkspaceConflictListQueryResult,
});
export type WorkspaceConflictListQueryResultEnvelope =
	typeof WorkspaceConflictListQueryResultEnvelope.Type;

/** Requests the unified diff for one recorded workspace change. */
export const WorkspaceChangeDiffQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.change.diff.query"),
	payload: WorkspaceChangeDiffQuery,
});

export type WorkspaceChangeDiffQueryEnvelope = typeof WorkspaceChangeDiffQueryEnvelope.Type;

/** Returns one correlated unified workspace-change diff. */
export const WorkspaceChangeDiffQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("workspace.change.diff.query.result"),
	payload: WorkspaceChangeDiffQueryResult,
});

export type WorkspaceChangeDiffQueryResultEnvelope =
	typeof WorkspaceChangeDiffQueryResultEnvelope.Type;

/** Requests the durable Git projection and unresolved mutations for one workspace. */
export const GitWorkspaceQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("git.workspace.query"),
	payload: GitWorkspaceQuery,
});

export type GitWorkspaceQueryEnvelope = typeof GitWorkspaceQueryEnvelope.Type;

/** Returns one correlated durable Git workspace projection. */
export const GitWorkspaceQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("git.workspace.query.result"),
	payload: GitWorkspaceQueryResult,
});

export type GitWorkspaceQueryResultEnvelope = typeof GitWorkspaceQueryResultEnvelope.Type;

/** Requests one bounded Git diff for an exact observed workspace snapshot. */
export const GitDiffQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("git.diff.query"),
	payload: GitDiffQuery,
});

export type GitDiffQueryEnvelope = typeof GitDiffQueryEnvelope.Type;

/** Returns one correlated ephemeral Git diff. */
export const GitDiffQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("git.diff.query.result"),
	payload: GitDiffQueryResult,
});

export type GitDiffQueryResultEnvelope = typeof GitDiffQueryResultEnvelope.Type;

/** Requests approval for staging exact paths with complete trace attribution. */
export const GitIndexStageRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	agent_id: Schema.optional(Identifier),
	kind: Schema.Literal("git.index.stage.request"),
	payload: GitIndexStageRequest,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
});

export type GitIndexStageRequestEnvelope = typeof GitIndexStageRequestEnvelope.Type;

/** Requests approval for unstaging exact paths with complete trace attribution. */
export const GitIndexUnstageRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	agent_id: Schema.optional(Identifier),
	kind: Schema.Literal("git.index.unstage.request"),
	payload: GitIndexUnstageRequest,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
});

export type GitIndexUnstageRequestEnvelope = typeof GitIndexUnstageRequestEnvelope.Type;

/** Resolves the approval bound to one exact Git mutation. */
export const GitMutationResolveEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	agent_id: Schema.optional(Identifier),
	kind: Schema.Literal("git.mutation.resolve"),
	payload: GitMutationResolveRequest,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
});

export type GitMutationResolveEnvelope = typeof GitMutationResolveEnvelope.Type;

/** Requests the curated Model Behaviour registry and current reconciliation state. */
export const ModelBehaviourQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("model_behaviour.query"),
	payload: Schema.Struct({}),
});

export type ModelBehaviourQueryEnvelope = typeof ModelBehaviourQueryEnvelope.Type;

/** Returns canonical controls and content-free provider reconciliation metadata. */
export const ModelBehaviourQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("model_behaviour.query.result"),
	payload: ModelBehaviourSnapshot,
});

export type ModelBehaviourQueryResultEnvelope = typeof ModelBehaviourQueryResultEnvelope.Type;

/** Replaces one canonical global model behavior and reconciles capable providers. */
export const ModelBehaviourUpdateEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("model_behaviour.update"),
	payload: ModelBehaviourUpdateRequest,
});

export type ModelBehaviourUpdateEnvelope = typeof ModelBehaviourUpdateEnvelope.Type;

/** Resolves one exact provider-native drift observation. */
export const ModelBehaviourDriftResolutionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("model_behaviour.drift.resolve"),
	payload: ModelBehaviourDriftResolutionRequest,
});

export type ModelBehaviourDriftResolutionEnvelope =
	typeof ModelBehaviourDriftResolutionEnvelope.Type;

/** Retries one provider mapping without changing the canonical setting. */
export const ModelBehaviourRetryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("model_behaviour.sync.retry"),
	payload: ModelBehaviourRetryRequest,
});

export type ModelBehaviourRetryEnvelope = typeof ModelBehaviourRetryEnvelope.Type;
