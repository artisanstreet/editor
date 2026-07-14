import { Schema } from "effect";

import { Identifier, IsoDateTime } from "./common";

/** Keeps automatic local Git fetch disabled until the user explicitly enables it. */
export const workspace_git_fetch_default_enabled = false;

/** Updates the one global policy controlling automatic local Git fetch. */
export const WorkspaceGitFetchPolicyUpdate = Schema.Struct({
	enabled: Schema.Boolean,
});

export type WorkspaceGitFetchPolicyUpdate = typeof WorkspaceGitFetchPolicyUpdate.Type;

/** Describes the provider-neutral outcome of one local Git fetch attempt. */
export const WorkspaceGitFetchResult = Schema.Literals(["succeeded", "failed", "unavailable"]);

export type WorkspaceGitFetchResult = typeof WorkspaceGitFetchResult.Type;

/** Projects the latest bounded fetch attempt for one workspace. */
export const WorkspaceGitFetchAttempt = Schema.Struct({
	attempted_at: IsoDateTime,
	result: WorkspaceGitFetchResult,
});

export type WorkspaceGitFetchAttempt = typeof WorkspaceGitFetchAttempt.Type;

/** Projects fetch state for one workspace without native or provider data. */
export const WorkspaceGitFetchWorkspaceState = Schema.Struct({
	last_attempt: Schema.optional(WorkspaceGitFetchAttempt),
	workspace_id: Identifier,
});

export type WorkspaceGitFetchWorkspaceState = typeof WorkspaceGitFetchWorkspaceState.Type;

/** Queries the global fetch policy and bounded workspace fetch states. */
export const WorkspaceGitFetchQuery = Schema.Struct({});

export type WorkspaceGitFetchQuery = typeof WorkspaceGitFetchQuery.Type;

/** Returns the global fetch policy and each known workspace's latest attempt. */
export const WorkspaceGitFetchQueryResult = Schema.Struct({
	enabled: Schema.Boolean,
	workspaces: Schema.Array(WorkspaceGitFetchWorkspaceState),
});

export type WorkspaceGitFetchQueryResult = typeof WorkspaceGitFetchQueryResult.Type;

/** Requests one manual fetch for the thread's selected workspace. */
export const WorkspaceGitFetchRequest = Schema.Struct({ workspace_id: Identifier });

export type WorkspaceGitFetchRequest = typeof WorkspaceGitFetchRequest.Type;
