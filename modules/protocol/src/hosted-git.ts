import { Schema } from "effect";

import { GitBranchName, GitObjectId } from "./git-session";
import { Identifier, IsoDateTime, JournalSequence, PositiveInt } from "./common";

const text_encoder = new TextEncoder();

function bounded_text(maximum_bytes: number, allow_lines = false) {
	return Schema.String.check(
		Schema.makeFilter<string>((value) => {
			const invalid_controls = Array.from(value).some(
				(character) =>
					(!allow_lines || !["\t", "\n", "\r"].includes(character)) &&
					/[\p{Cc}\p{Cf}]/u.test(character),
			);

			return value.trim().length === 0 ||
				text_encoder.encode(value).byteLength > maximum_bytes ||
				invalid_controls
				? `Expected non-empty text bounded to ${maximum_bytes} bytes`
				: undefined;
		}),
	);
}

const HostedGitProviderId = Schema.String.check(
	Schema.isPattern(/^[a-z][a-z0-9_-]{0,63}$/u, {
		message: "Expected a canonical hosted Git provider ID",
	}),
);

const HostedGitHost = Schema.String.check(
	Schema.makeFilter<string>((value) => {
		if (
			value.length === 0 ||
			value !== value.trim() ||
			/[\p{Cc}\p{Cf}/?#@]/u.test(value) ||
			!URL.canParse(`https://${value}`)
		) {
			return "Expected a canonical hosted Git host";
		}

		const parsed = URL.parse(`https://${value}`);

		return parsed === null || parsed.host !== value || parsed.pathname !== "/"
			? "Expected a canonical hosted Git host"
			: undefined;
	}),
);

const HostedGitLogin = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.trim().length === 0 ||
		text_encoder.encode(value).byteLength > 128 ||
		/[\p{Cc}\p{Cf}\s]/u.test(value)
			? "Expected a bounded hosted Git login"
			: undefined,
	),
);

const HostedGitNativeId = bounded_text(512);
const HostedGitName = bounded_text(512);
const HostedGitTitle = bounded_text(1_024);
const HostedGitPath = bounded_text(4_096);
const HostedGitUntrustedText = bounded_text(4_096, true);
const HostedGitUntrustedLogExcerpt = bounded_text(64 * 1_024, true);

const HostedGitWebUrl = Schema.String.check(
	Schema.makeFilter<string>((value) => {
		if (
			text_encoder.encode(value).byteLength > 2_048 ||
			/[\p{Cc}\p{Cf}]/u.test(value) ||
			!URL.canParse(value)
		) {
			return "Expected a bounded hosted Git web URL";
		}

		const parsed = URL.parse(value);

		return parsed === null ||
			(parsed.protocol !== "https:" && parsed.protocol !== "http:") ||
			parsed.hostname.length === 0 ||
			parsed.username !== "" ||
			parsed.password !== "" ||
			parsed.hash !== ""
			? "Expected a hosted Git web URL without credentials or fragments"
			: undefined;
	}),
);

/** Identifies the hosted repository whose selected branch was inspected. */
export const HostedGitRepositoryIdentity = Schema.Struct({
	host: HostedGitHost,
	name: HostedGitName,
	owner: HostedGitLogin,
	provider_id: HostedGitProviderId,
});

export type HostedGitRepositoryIdentity = typeof HostedGitRepositoryIdentity.Type;

/** Preserves a minimal provider-native identity for one canonical hosted resource. */
export const HostedGitOrigin = Schema.Struct({
	native_id: HostedGitNativeId,
	provider_id: HostedGitProviderId,
	resource_kind: Schema.Literals([
		"pull_request",
		"review",
		"review_thread",
		"check_run",
		"status_context",
		"check_suite",
		"workflow_run",
	]),
});

export type HostedGitOrigin = typeof HostedGitOrigin.Type;

/** Identifies a requested user or team reviewer without provider payload leakage. */
export const HostedGitRequestedReviewer = Schema.Union([
	Schema.Struct({
		_tag: Schema.Literal("user"),
		login: HostedGitLogin,
	}),
	Schema.Struct({
		_tag: Schema.Literal("team"),
		organization: HostedGitLogin,
		slug: HostedGitLogin,
	}),
]);

export type HostedGitRequestedReviewer = typeof HostedGitRequestedReviewer.Type;

/** Projects one submitted review without importing its untrusted body as instructions. */
export const HostedGitReview = Schema.Struct({
	author: Schema.optional(HostedGitLogin),
	commit: Schema.optional(GitObjectId),
	origin: HostedGitOrigin,
	state: Schema.Literals(["approved", "changes_requested", "commented", "dismissed"]),
	submitted_at: IsoDateTime,
});

export type HostedGitReview = typeof HostedGitReview.Type;

/** Projects one review thread and the last provider revision that can wake a durable watch. */
export const HostedGitReviewThread = Schema.Struct({
	comment_count: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	last_comment_native_id: Schema.optional(HostedGitNativeId),
	last_updated_at: Schema.optional(IsoDateTime),
	line: Schema.optional(Schema.Int.check(Schema.isGreaterThanOrEqualTo(1))),
	origin: HostedGitOrigin,
	outdated: Schema.Boolean,
	path: HostedGitPath,
	resolved: Schema.Boolean,
	subject: Schema.Literals(["file", "line"]),
});

export type HostedGitReviewThread = typeof HostedGitReviewThread.Type;

/** Carries bounded provider text as attributed, explicitly untrusted check output. */
export const HostedGitCheckAnnotation = Schema.Struct({
	end_line: Schema.Int.check(Schema.isGreaterThanOrEqualTo(1)),
	level: Schema.Literals(["notice", "warning", "failure"]),
	path: HostedGitPath,
	start_line: Schema.Int.check(Schema.isGreaterThanOrEqualTo(1)),
	title: Schema.optional(HostedGitName),
	untrusted_message: HostedGitUntrustedText,
}).check(
	Schema.makeFilter((annotation) =>
		annotation.end_line < annotation.start_line
			? "Expected the annotation end line to follow its start line"
			: undefined,
	),
);

export type HostedGitCheckAnnotation = typeof HostedGitCheckAnnotation.Type;

/** Normalizes provider-native check and commit-status lifecycles for UI and wait policy. */
export const HostedGitCheckState = Schema.Literals([
	"queued",
	"running",
	"passed",
	"failed",
	"cancelled",
	"skipped",
	"stale",
	"action_required",
	"neutral",
	"timed_out",
	"unknown",
]);

export type HostedGitCheckState = typeof HostedGitCheckState.Type;

/** Projects one check job or legacy status context for the exact pull-request head. */
export const HostedGitCheck = Schema.Struct({
	annotations: Schema.Array(HostedGitCheckAnnotation),
	annotations_truncated: Schema.Boolean,
	app_name: Schema.optional(HostedGitName),
	attempt: Schema.optional(PositiveInt),
	completed_at: Schema.optional(IsoDateTime),
	details_url: Schema.optional(HostedGitWebUrl),
	name: HostedGitName,
	origin: HostedGitOrigin,
	required: Schema.Boolean,
	started_at: Schema.optional(IsoDateTime),
	state: HostedGitCheckState,
	suite_origin: Schema.optional(HostedGitOrigin),
	workflow_name: Schema.optional(HostedGitName),
	workflow_origin: Schema.optional(HostedGitOrigin),
	workflow_url: Schema.optional(HostedGitWebUrl),
});

export type HostedGitCheck = typeof HostedGitCheck.Type;

/** Projects bounded provider-owned output for one failed check without treating it as instructions. */
export const HostedGitCheckFailureOutput = Schema.Struct({
	title: Schema.optional(HostedGitTitle),
	untrusted_summary: Schema.optional(HostedGitUntrustedText),
	untrusted_text: Schema.optional(HostedGitUntrustedText),
});

export type HostedGitCheckFailureOutput = typeof HostedGitCheckFailureOutput.Type;

/** Distinguishes a bounded failed-job excerpt from a truthful unavailable state. */
export const HostedGitCheckFailureLog = Schema.Union([
	Schema.Struct({
		_tag: Schema.Literal("available"),
		observed_bytes: Schema.Int.check(Schema.isGreaterThanOrEqualTo(1)),
		truncated: Schema.Boolean,
		untrusted_excerpt: HostedGitUntrustedLogExcerpt,
	}),
	Schema.Struct({
		_tag: Schema.Literal("unavailable"),
		reason: Schema.Literals(["check_not_completed", "not_actions_job", "not_available"]),
	}),
]);

export type HostedGitCheckFailureLog = typeof HostedGitCheckFailureLog.Type;

/** Returns fresh, bounded failure detail for one exact check run and hosted head. */
export const HostedGitCheckFailureDetail = Schema.Struct({
	attempt: Schema.optional(PositiveInt),
	check_origin: HostedGitOrigin,
	head_commit: GitObjectId,
	log: HostedGitCheckFailureLog,
	name: HostedGitName,
	output: HostedGitCheckFailureOutput,
	workflow_origin: Schema.optional(HostedGitOrigin),
}).check(
	Schema.makeFilter((detail) =>
		detail.check_origin.resource_kind !== "check_run" ||
		(detail.workflow_origin !== undefined &&
			detail.workflow_origin.resource_kind !== "workflow_run") ||
		(detail.workflow_origin !== undefined &&
			detail.workflow_origin.provider_id !== detail.check_origin.provider_id)
			? "Expected check-run detail with an optional workflow from the same provider"
			: undefined,
	),
);

export type HostedGitCheckFailureDetail = typeof HostedGitCheckFailureDetail.Type;

const HostedGitPullRequestSummaryFields = {
	base_branch: GitBranchName,
	draft: Schema.Boolean,
	head_branch: GitBranchName,
	head_commit: GitObjectId,
	number: PositiveInt,
	origin: HostedGitOrigin,
	state: Schema.Literals(["open", "closed", "merged"]),
	title: HostedGitTitle,
	web_url: HostedGitWebUrl,
};

/** Summarizes a pull request when one selected branch has multiple valid associations. */
export const HostedGitPullRequestSummary = Schema.Struct(HostedGitPullRequestSummaryFields);

export type HostedGitPullRequestSummary = typeof HostedGitPullRequestSummary.Type;

/** Projects review and CI state for one exact provider pull request. */
export const HostedGitPullRequest = Schema.Struct({
	...HostedGitPullRequestSummaryFields,
	base_commit: GitObjectId,
	checks: Schema.Array(HostedGitCheck),
	checks_total: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	checks_truncated: Schema.Boolean,
	mergeability: Schema.Literals(["mergeable", "conflicting", "unknown"]),
	requested_reviewers: Schema.Array(HostedGitRequestedReviewer),
	requested_reviewers_truncated: Schema.Boolean,
	review_decision: Schema.Literals(["approved", "changes_requested", "review_required", "none"]),
	review_threads: Schema.Array(HostedGitReviewThread),
	review_threads_total: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	review_threads_truncated: Schema.Boolean,
	reviews: Schema.Array(HostedGitReview),
	reviews_total: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	reviews_truncated: Schema.Boolean,
});

export type HostedGitPullRequest = typeof HostedGitPullRequest.Type;

/** Represents branch association and exact-head freshness without guessing from titles or transcripts. */
export const HostedGitPullRequestAssociation = Schema.Union([
	Schema.Struct({ _tag: Schema.Literal("none") }),
	Schema.Struct({
		_tag: Schema.Literal("ambiguous"),
		candidates: Schema.Array(HostedGitPullRequestSummary).check(
			Schema.isMinLength(2),
			Schema.isMaxLength(10),
		),
		candidates_truncated: Schema.Boolean,
	}),
	Schema.Struct({
		_tag: Schema.Literal("matched"),
		freshness: Schema.Literals(["current", "stale_head"]),
		pull_request: HostedGitPullRequest,
	}),
]);

export type HostedGitPullRequestAssociation = typeof HostedGitPullRequestAssociation.Type;

/** Returns canonical hosted state bound to the branch and local head used for the read. */
export const HostedGitPullRequestLookup = Schema.Struct({
	association: HostedGitPullRequestAssociation,
	branch: GitBranchName,
	expected_head_commit: GitObjectId,
	repository: HostedGitRepositoryIdentity,
});

export type HostedGitPullRequestLookup = typeof HostedGitPullRequestLookup.Type;

/** Persists one provider read with unverified freshness until a query checks live local Git. */
export const HostedGitSnapshot = Schema.Struct({
	journal_sequence: JournalSequence,
	lookup: HostedGitPullRequestLookup,
	observed_at: IsoDateTime,
	project_id: Identifier,
	version: PositiveInt,
	workspace_freshness: Schema.Literals(["unverified", "current", "stale_local_git"]),
	workspace_id: Identifier,
});

export type HostedGitSnapshot = typeof HostedGitSnapshot.Type;

/** Requests the latest durable hosted review and CI projection for one workspace. */
export const HostedGitSnapshotQuery = Schema.Struct({ workspace_id: Identifier });

export type HostedGitSnapshotQuery = typeof HostedGitSnapshotQuery.Type;

/** Returns an optional hosted projection at the latest durable journal position. */
export const HostedGitSnapshotQueryResult = Schema.Struct({
	journal_sequence: JournalSequence,
	snapshot: Schema.optional(HostedGitSnapshot),
});

export type HostedGitSnapshotQueryResult = typeof HostedGitSnapshotQueryResult.Type;

/** Requests a fresh exact-head hosted review and CI observation. */
export const HostedGitSnapshotRefreshRequest = Schema.Struct({ workspace_id: Identifier });

export type HostedGitSnapshotRefreshRequest = typeof HostedGitSnapshotRefreshRequest.Type;

/** Announces one durable projection whose local-workspace freshness remains unverified. */
export const HostedGitSnapshotUpdatedEvent = Schema.Struct({
	snapshot: HostedGitSnapshot,
	type: Schema.Literal("hosted.git.snapshot.updated"),
});

export type HostedGitSnapshotUpdatedEvent = typeof HostedGitSnapshotUpdatedEvent.Type;
