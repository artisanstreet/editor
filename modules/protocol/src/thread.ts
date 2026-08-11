import { Schema } from "effect";

import { Identifier, IsoDateTime, StreamSequence } from "./common";

/** Enumerates meaningful actions that advance thread retention activity. */
export const ThreadActivityKind = Schema.Literals([
	"user_message",
	"run_started",
	"run_completed",
	"run_failed",
	"file_attached",
	"diff_attached",
	"terminal_attached",
	"process_attached",
	"renamed",
	"pinned",
	"unpinned",
	"archived",
	"restored",
]);

export type ThreadActivityKind = typeof ThreadActivityKind.Type;

/** Identifies how the current thread title was established. */
export const ThreadTitleSource = Schema.Literals(["initial", "automatic", "manual"]);

export type ThreadTitleSource = typeof ThreadTitleSource.Type;

/** Identifies one canonical local workspace project. */
export const ProjectRef = Schema.Struct({
	display_name: Schema.NonEmptyString,
	project_id: Identifier,
	root_path: Schema.NonEmptyString,
});

export type ProjectRef = typeof ProjectRef.Type;

/** Enumerates content-free evidence categories used by deterministic scoring. */
export const ProjectAffinityEvidenceKind = Schema.Literals([
	"active_working_directory",
	"file_artifact",
	"file_mutation",
	"git_branch",
	"git_diff",
	"git_root",
	"git_worktree",
	"historical_working_directory",
	"process_owner",
	"project_mention",
	"terminal_working_directory",
	"thread_metadata",
]);

export type ProjectAffinityEvidenceKind = typeof ProjectAffinityEvidenceKind.Type;

/** Counts unique durable evidence facts contributing to one project score. */
export const ProjectAffinityEvidenceCount = Schema.Struct({
	count: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	kind: ProjectAffinityEvidenceKind,
});

export type ProjectAffinityEvidenceCount = typeof ProjectAffinityEvidenceCount.Type;

/** Projects one deterministic affinity score for a candidate project. */
export const ProjectAffinityScore = Schema.Struct({
	evidence: Schema.Array(ProjectAffinityEvidenceCount),
	project: ProjectRef,
	score: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(100)),
});

export type ProjectAffinityScore = typeof ProjectAffinityScore.Type;

/** Suggests a scored project move against one exact affinity projection version. */
export const ThreadProjectRehomeSuggestion = Schema.Struct({
	basis_affinity_version: StreamSequence,
	project: ProjectRef,
	score: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(100)),
});

export type ThreadProjectRehomeSuggestion = typeof ThreadProjectRehomeSuggestion.Type;

/** Bounds the assistant prose copied into compact thread-list surfaces. */
export const ThreadAssistantMessagePreviewMaximumLength = 500;

export const ThreadAssistantMessagePreview = Schema.NonEmptyString.check(
	Schema.isMaxLength(ThreadAssistantMessagePreviewMaximumLength),
);

/** Describes the durable thread projection sent to sidebar clients. */
export const ThreadListItem = Schema.Struct({
	activity_version: StreamSequence,
	affinity_version: StreamSequence,
	archived_at: Schema.optional(IsoDateTime),
	created_at: IsoDateTime,
	current_goal: Schema.optional(Schema.NonEmptyString),
	/**
	 * The engine and model the thread's coordinator currently launches with, so
	 * a list can show what a thread is on without opening it. Absent until the
	 * thread has a coordinator, which it gains with its first message.
	 */
	engine_id: Schema.optional(Identifier),
	model_id: Schema.optional(Schema.NonEmptyString),
	last_activity_at: IsoDateTime,
	last_assistant_message: Schema.optional(ThreadAssistantMessagePreview),
	live_status: Schema.NonEmptyString,
	metadata_version: StreamSequence,
	pinned: Schema.Boolean,
	/**
	 * Newest activity exposed by the root thread surface. Hidden worker lifecycle
	 * still advances retention recency through `last_activity_at`, but cannot make
	 * the root conversation acknowledge content the reader never saw.
	 *
	 * Optional only for protocol compatibility with projections written before
	 * the participant boundary existed; current Forge projections always emit it.
	 */
	reader_activity_at: Schema.optional(IsoDateTime),
	/**
	 * Root-visible activity through which the reader has durably acknowledged
	 * the thread's attention request. Newer root activity deliberately repins it.
	 */
	reader_acknowledged_activity_at: Schema.optional(IsoDateTime),
	primary_project: Schema.optional(ProjectRef),
	project_affinity_scores: Schema.Array(ProjectAffinityScore),
	project_locked: Schema.Boolean,
	rename_suggestion: Schema.optional(Schema.NonEmptyString),
	rehome_suggestion: Schema.optional(ThreadProjectRehomeSuggestion),
	linked_projects: Schema.Array(ProjectRef),
	thread_id: Identifier,
	title: Schema.NonEmptyString,
	title_locked: Schema.Boolean,
	title_source: ThreadTitleSource,
	updated_at: IsoDateTime,
});

export type ThreadListItem = typeof ThreadListItem.Type;

/** Describes client intent for Forge-owned thread creation. */
export const ThreadCreateInput = Schema.Struct({
	project_id: Schema.optional(Identifier),
	title: Schema.NonEmptyString,
});

export type ThreadCreateInput = typeof ThreadCreateInput.Type;

/** Creates a durable thread with an initial auto-managed identity. */
export const ThreadCreateCommand = Schema.Struct({
	project_id: Schema.optional(Identifier),
	type: Schema.Literal("thread.create"),
	title: Schema.NonEmptyString,
});

export type ThreadCreateCommand = typeof ThreadCreateCommand.Type;

/** Permanently locks a thread title to the user's chosen value. */
export const ThreadRenameCommand = Schema.Struct({
	type: Schema.Literal("thread.rename"),
	title: Schema.NonEmptyString,
});

export type ThreadRenameCommand = typeof ThreadRenameCommand.Type;

/** Unconditionally pins a thread to the user's project in durable command order. */
export const ThreadProjectAssignCommand = Schema.Struct({
	project_id: Identifier,
	type: Schema.Literal("thread.project.assign"),
});

export type ThreadProjectAssignCommand = typeof ThreadProjectAssignCommand.Type;

/** Returns a manually pinned thread to automatic project-affinity management. */
export const ThreadProjectUnlockCommand = Schema.Struct({
	basis_affinity_version: StreamSequence,
	type: Schema.Literal("thread.project.unlock"),
});

export type ThreadProjectUnlockCommand = typeof ThreadProjectUnlockCommand.Type;

/** Applies asynchronous auto-managed metadata against explicit projection versions. */
export const ThreadMetadataRefineCommand = Schema.Struct({
	basis_activity_version: StreamSequence,
	basis_metadata_version: StreamSequence,
	current_goal: Schema.optional(Schema.NonEmptyString),
	last_assistant_message: Schema.optional(ThreadAssistantMessagePreview),
	live_status: Schema.NonEmptyString,
	mentioned_projects: Schema.optional(Schema.Array(ProjectRef)),
	rename_suggestion: Schema.optional(Schema.NonEmptyString),
	title: Schema.optional(Schema.NonEmptyString),
	type: Schema.Literal("thread.metadata.refine"),
});

export type ThreadMetadataRefineCommand = typeof ThreadMetadataRefineCommand.Type;

/** Records one meaningful activity that resets the retention horizon. */
export const ThreadActivityRecordCommand = Schema.Struct({
	activity_kind: ThreadActivityKind,
	type: Schema.Literal("thread.activity.record"),
});

export type ThreadActivityRecordCommand = typeof ThreadActivityRecordCommand.Type;

/** Acknowledges one observed root-visible activity cursor without rewriting history. */
export const ThreadAttentionAcknowledgeCommand = Schema.Struct({
	reader_activity_at: IsoDateTime,
	type: Schema.Literal("thread.attention.acknowledge"),
});

export type ThreadAttentionAcknowledgeCommand = typeof ThreadAttentionAcknowledgeCommand.Type;

/** Exempts a thread from automatic retention deletion. */
export const ThreadPinCommand = Schema.Struct({ type: Schema.Literal("thread.pin") });

/** Removes a thread's automatic-retention exemption. */
export const ThreadUnpinCommand = Schema.Struct({ type: Schema.Literal("thread.unpin") });

/** Moves a thread out of the active sidebar while preserving normal retention. */
export const ThreadArchiveCommand = Schema.Struct({ type: Schema.Literal("thread.archive") });

/** Returns an archived thread to the active sidebar. */
export const ThreadRestoreCommand = Schema.Struct({ type: Schema.Literal("thread.restore") });

/** Describes the intentionally small global inactive-thread retention setting. */
export const ThreadRetentionInactivityDays = Schema.Int.check(
	Schema.isGreaterThanOrEqualTo(1),
	Schema.isLessThanOrEqualTo(3650),
);

export const ThreadRetentionPolicy = Schema.Struct({
	enabled: Schema.Boolean,
	inactivity_days: ThreadRetentionInactivityDays,
});

export type ThreadRetentionPolicy = typeof ThreadRetentionPolicy.Type;

/** Updates the global inactive-thread retention policy through the durable command path. */
export const ThreadRetentionUpdateCommand = Schema.Struct({
	type: Schema.Literal("thread.retention.update"),
	...ThreadRetentionPolicy.fields,
});

export type ThreadRetentionUpdateCommand = typeof ThreadRetentionUpdateCommand.Type;

/** Defines the event payload emitted when a thread is created. */
export const ThreadCreatedEvent = Schema.Struct({
	type: Schema.Literal("thread.created"),
	title: Schema.NonEmptyString,
});

export type ThreadCreatedEvent = typeof ThreadCreatedEvent.Type;

/** Projects one durable thread identity, lifecycle, or meaningful activity change. */
export const ThreadMetadataUpdatedEvent = Schema.Struct({
	activity_kind: Schema.optional(ThreadActivityKind),
	change: Schema.Literals([
		"activity",
		"attention_acknowledged",
		"archive",
		"metadata",
		"pin",
		"rename",
		"restore",
		"unpin",
	]),
	mentioned_projects: Schema.optional(Schema.Array(ProjectRef)),
	thread: ThreadListItem,
	type: Schema.Literal("thread.metadata.updated"),
});

export type ThreadMetadataUpdatedEvent = typeof ThreadMetadataUpdatedEvent.Type;

/**
 * Replays an acknowledgement cursor without restating the complete thread
 * projection. Used by durable upgrades of projections written before readers
 * could acknowledge attention.
 */
export const ThreadAttentionAcknowledgedEvent = Schema.Struct({
	reader_activity_at: IsoDateTime,
	type: Schema.Literal("thread.attention.acknowledged"),
});

export type ThreadAttentionAcknowledgedEvent = typeof ThreadAttentionAcknowledgedEvent.Type;

/** Records an accepted refinement result that lost its projection-version race. */
export const ThreadRefinementIgnoredEvent = Schema.Struct({
	basis_activity_version: StreamSequence,
	basis_metadata_version: StreamSequence,
	type: Schema.Literal("thread.refinement.ignored"),
});

export type ThreadRefinementIgnoredEvent = typeof ThreadRefinementIgnoredEvent.Type;

/** Records a durable replacement of a thread's project-affinity projection. */
export const ThreadProjectAffinityUpdatedEvent = Schema.Struct({
	change: Schema.Literals(["assigned", "observed", "rehomed", "suggested", "unlocked"]),
	thread: ThreadListItem,
	type: Schema.Literal("thread.project_affinity.updated"),
});

export type ThreadProjectAffinityUpdatedEvent = typeof ThreadProjectAffinityUpdatedEvent.Type;

/** Records an affinity input that could not alter the current projection. */
export const ThreadProjectAffinityIgnoredEvent = Schema.Struct({
	basis_affinity_version: StreamSequence,
	reason: Schema.Literals(["locked", "stale_basis"]),
	type: Schema.Literal("thread.project_affinity.ignored"),
});

export type ThreadProjectAffinityIgnoredEvent = typeof ThreadProjectAffinityIgnoredEvent.Type;

/** Records a durable update to the global inactive-thread retention policy. */
export const ThreadRetentionPolicyUpdatedEvent = Schema.Struct({
	policy: ThreadRetentionPolicy,
	type: Schema.Literal("thread.retention.updated"),
});

export type ThreadRetentionPolicyUpdatedEvent = typeof ThreadRetentionPolicyUpdatedEvent.Type;

/** Replaces erased historical thread content while preserving replay coordinates. */
export const ThreadContentErasedEvent = Schema.Struct({
	type: Schema.Literal("thread.content_erased"),
});

export type ThreadContentErasedEvent = typeof ThreadContentErasedEvent.Type;

/** Marks the durable terminal point of an erased thread stream. */
export const ThreadErasedEvent = Schema.Struct({
	type: Schema.Literal("thread.erased"),
});

export type ThreadErasedEvent = typeof ThreadErasedEvent.Type;
