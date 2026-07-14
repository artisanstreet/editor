import { Schema } from "effect";

import { GitBranchName, GitObjectId } from "./git-session";
import { Identifier, IsoDateTime, JournalSequence, PositiveInt } from "./common";
import { HostedGitOrigin, HostedGitRepositoryIdentity } from "./hosted-git";

const text_encoder = new TextEncoder();

const BoundedExternalText = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.trim().length === 0 ||
		text_encoder.encode(value).byteLength > 512 ||
		/[\p{Cc}\p{Cf}]/u.test(value)
			? "Expected non-empty bounded external text without control characters"
			: undefined,
	),
);

const BoundedExternalIdentifier = Identifier.check(
	Schema.makeFilter<string>((value) =>
		text_encoder.encode(value).byteLength > 256 || /[\p{Cc}\p{Cf}]/u.test(value)
			? "Expected a bounded identifier without control or format characters"
			: undefined,
	),
);

const BaselineFingerprint = Schema.String.check(
	Schema.isPattern(/^[a-f0-9]{64}$/u, {
		message: "Expected a lowercase SHA-256 baseline fingerprint",
	}),
);

function has_unique_values(values: ReadonlyArray<string>): boolean {
	return new Set(values).size === values.length;
}

function gate_identity(gate: ExternalWaitGate): string {
	return gate._tag;
}

const ExternalWaitGateBase = Schema.Union([
	Schema.Struct({ _tag: Schema.Literal("required_checks_terminal") }),
	Schema.Struct({
		_tag: Schema.Literal("selected_checks_terminal"),
		check_names: Schema.Array(BoundedExternalText).check(
			Schema.isMinLength(1),
			Schema.isMaxLength(32),
		),
	}),
	Schema.Struct({ _tag: Schema.Literal("review_decision_changed") }),
	Schema.Struct({ _tag: Schema.Literal("review_submitted") }),
	Schema.Struct({ _tag: Schema.Literal("review_threads_changed") }),
]);

/** Defines one provider-neutral hosted review or CI condition that can wake a run. */
export const ExternalWaitGate = ExternalWaitGateBase.check(
	Schema.makeFilter<typeof ExternalWaitGateBase.Type>((gate) =>
		gate._tag === "selected_checks_terminal" && !has_unique_values(gate.check_names)
			? "Expected selected check names to be unique"
			: undefined,
	),
);

export type ExternalWaitGate = typeof ExternalWaitGate.Type;

const ExternalWaitGates = Schema.Array(ExternalWaitGate).check(
	Schema.isMinLength(1),
	Schema.isMaxLength(8),
	Schema.makeFilter<ReadonlyArray<ExternalWaitGate>>((gates) =>
		has_unique_values(gates.map(gate_identity))
			? undefined
			: "Expected external wait gates to be unique",
	),
);

/** Identifies the durable Artisan run that owns an external wait. */
export const ExternalWaitOwner = Schema.Union([
	Schema.Struct({
		_tag: Schema.Literal("thread_run"),
		agent_id: BoundedExternalIdentifier,
		engine_id: BoundedExternalIdentifier,
		run_id: BoundedExternalIdentifier,
	}),
	Schema.Struct({
		_tag: Schema.Literal("assignment_run"),
		agent_id: BoundedExternalIdentifier,
		assignment_id: BoundedExternalIdentifier,
		engine_id: BoundedExternalIdentifier,
		group_id: BoundedExternalIdentifier,
		run_id: BoundedExternalIdentifier,
	}),
]);

export type ExternalWaitOwner = typeof ExternalWaitOwner.Type;

/** Binds a wait to one exact hosted pull-request head and durable repository identity. */
const ExternalWaitTargetBase = Schema.Struct({
	branch: GitBranchName.check(
		Schema.makeFilter<string>((value) =>
			/[\p{Cf}]/u.test(value)
				? "Expected a branch name without format characters"
				: undefined,
		),
	),
	expected_head_commit: GitObjectId,
	pull_request_number: PositiveInt,
	pull_request_origin: HostedGitOrigin,
	repository: HostedGitRepositoryIdentity,
});

/** Binds a wait to one exact hosted pull-request head and durable repository identity. */
export const ExternalWaitTarget = ExternalWaitTargetBase.check(
	Schema.makeFilter<typeof ExternalWaitTargetBase.Type>((target) => {
		if (target.pull_request_origin.resource_kind !== "pull_request") {
			return "Expected a pull request origin";
		}

		return target.pull_request_origin.provider_id === target.repository.provider_id
			? undefined
			: "Expected the pull request origin and repository provider to match";
	}),
);

export type ExternalWaitTarget = typeof ExternalWaitTarget.Type;

/** Accepts only the durable inputs needed to derive a wait owner and hosted target. */
export const ExternalWaitRequest = Schema.Struct({
	expected_head_commit: GitObjectId,
	gates: ExternalWaitGates,
	pull_request_number: PositiveInt,
	source_run_id: BoundedExternalIdentifier,
	workspace_id: BoundedExternalIdentifier,
});

export type ExternalWaitRequest = typeof ExternalWaitRequest.Type;

/** Requests cancellation of one durable external wait. */
export const ExternalWaitCancelRequest = Schema.Struct({ wait_id: BoundedExternalIdentifier });

export type ExternalWaitCancelRequest = typeof ExternalWaitCancelRequest.Type;

/** Requests a user-triggered resume of one durable external wait. */
export const ExternalWaitManualResumeRequest = Schema.Struct({
	wait_id: BoundedExternalIdentifier,
});

export type ExternalWaitManualResumeRequest = typeof ExternalWaitManualResumeRequest.Type;

/** Requests the durable external wait attached to one thread. */
export const ExternalWaitQuery = Schema.Struct({ thread_id: BoundedExternalIdentifier });

export type ExternalWaitQuery = typeof ExternalWaitQuery.Type;

const ExternalWaitQueryResultBase = Schema.Struct({
	snapshots: Schema.Array(Schema.suspend(() => ExternalWaitSnapshot)).check(
		Schema.isMaxLength(64),
	),
	truncated: Schema.Boolean,
});

/** Returns every bounded durable external wait attached to one thread. */
export const ExternalWaitQueryResult = ExternalWaitQueryResultBase.check(
	Schema.makeFilter<typeof ExternalWaitQueryResultBase.Type>((result) => {
		const wait_ids = result.snapshots.map((snapshot) => snapshot.wait_id);
		const thread_ids = new Set(result.snapshots.map((snapshot) => snapshot.thread_id));

		return new Set(wait_ids).size === wait_ids.length && thread_ids.size <= 1
			? undefined
			: "Expected unique external waits from one thread";
	}),
);

export type ExternalWaitQueryResult = typeof ExternalWaitQueryResult.Type;

/** Summarizes one terminal hosted check without provider prose, logs, or annotations. */
export const ExternalWaitCheckSummary = Schema.Struct({
	name: BoundedExternalText,
	required: Schema.Boolean,
	state: Schema.Literals([
		"passed",
		"failed",
		"cancelled",
		"skipped",
		"action_required",
		"neutral",
		"timed_out",
	]),
	workflow_name: Schema.optional(BoundedExternalText),
});

export type ExternalWaitCheckSummary = typeof ExternalWaitCheckSummary.Type;

const ExternalWaitCheckSummaries = Schema.Array(ExternalWaitCheckSummary).check(
	Schema.isMinLength(1),
	Schema.isMaxLength(64),
);

/** Describes a canonical terminal-check transition that may wake a durable wait. */
export const ExternalWaitChecksTerminalTrigger = Schema.Struct({
	check_summaries: ExternalWaitCheckSummaries,
	truncated: Schema.Literal(false),
	_tag: Schema.Literal("checks_terminal"),
});

export type ExternalWaitChecksTerminalTrigger = typeof ExternalWaitChecksTerminalTrigger.Type;

/** Describes a canonical review transition that may wake a durable wait. */
export const ExternalWaitReviewChangedTrigger = Schema.Struct({
	change_kind: Schema.Literals(["decision_changed", "review_submitted", "threads_changed"]),
	decision: Schema.Literals(["approved", "changes_requested", "review_required", "none"]),
	total_reviews: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	unresolved_thread_count: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	_tag: Schema.Literal("review_changed"),
});

export type ExternalWaitReviewChangedTrigger = typeof ExternalWaitReviewChangedTrigger.Type;

/** Identifies a user-requested wake without carrying external provider content. */
export const ExternalWaitManualResumeTrigger = Schema.Struct({
	_tag: Schema.Literal("manual_resume"),
});

export type ExternalWaitManualResumeTrigger = typeof ExternalWaitManualResumeTrigger.Type;

/** Describes one safe public cause for waking an external wait. */
export const ExternalWaitTrigger = Schema.Union([
	ExternalWaitChecksTerminalTrigger,
	ExternalWaitReviewChangedTrigger,
	ExternalWaitManualResumeTrigger,
]);

export type ExternalWaitTrigger = typeof ExternalWaitTrigger.Type;

/** Represents the public lifecycle of a durable external wait. */
export const ExternalWaitState = Schema.Union([
	Schema.Struct({ _tag: Schema.Literal("waiting") }),
	Schema.Struct({ _tag: Schema.Literal("wake_pending"), trigger: ExternalWaitTrigger }),
	Schema.Struct({
		_tag: Schema.Literal("woken"),
		follow_up_run_id: BoundedExternalIdentifier,
		mode: Schema.Literals(["native_resume", "linked_run"]),
		trigger: ExternalWaitTrigger,
	}),
	Schema.Struct({
		_tag: Schema.Literal("suspended"),
		reason: Schema.Literals([
			"stale_head",
			"authentication_required",
			"rate_limited",
			"provider_unavailable",
			"project_unavailable",
			"timeout",
		]),
	}),
	Schema.Struct({
		_tag: Schema.Literal("cancelled"),
		reason: Schema.Literals(["user", "thread_terminal", "project_removed", "superseded"]),
	}),
	Schema.Struct({ _tag: Schema.Literal("exhausted") }),
]);

export type ExternalWaitState = typeof ExternalWaitState.Type;

/** Projects the complete source-free durable state of one external wait. */
export const ExternalWaitSnapshot = Schema.Struct({
	baseline_fingerprint: BaselineFingerprint,
	created_at: IsoDateTime,
	gates: ExternalWaitGates,
	generation: PositiveInt,
	maximum_generation: PositiveInt,
	owner: ExternalWaitOwner,
	project_id: BoundedExternalIdentifier,
	state: ExternalWaitState,
	target: ExternalWaitTarget,
	thread_id: BoundedExternalIdentifier,
	updated_at: IsoDateTime,
	version: PositiveInt,
	wait_id: BoundedExternalIdentifier,
	workspace_id: BoundedExternalIdentifier,
	journal_sequence: JournalSequence,
}).check(
	Schema.makeFilter<typeof ExternalWaitSnapshot.Type>((snapshot) =>
		snapshot.generation > snapshot.maximum_generation
			? "Expected generation not to exceed maximum generation"
			: undefined,
	),
);

export type ExternalWaitSnapshot = typeof ExternalWaitSnapshot.Type;

/** Announces one source-free durable external-wait projection update. */
export const ExternalWaitUpdatedEvent = Schema.Struct({
	snapshot: ExternalWaitSnapshot,
	type: Schema.Literal("external_wait.updated"),
});

export type ExternalWaitUpdatedEvent = typeof ExternalWaitUpdatedEvent.Type;
