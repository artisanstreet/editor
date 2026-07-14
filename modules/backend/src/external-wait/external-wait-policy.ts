import { Data, Effect, Match, Schema } from "effect";

import {
	ExternalWaitChecksTerminalTrigger,
	ExternalWaitRequest,
	ExternalWaitReviewChangedTrigger,
	ExternalWaitTarget,
	HostedGitCheck,
	HostedGitOrigin,
	HostedGitPullRequestLookup,
	HostedGitPullRequest,
	HostedGitReview,
	HostedGitReviewThread,
	type ExternalWaitCheckSummary,
	type ExternalWaitChecksTerminalTrigger as ExternalWaitChecksTerminalTriggerValue,
	type ExternalWaitGate as ExternalWaitGateValue,
	type ExternalWaitReviewChangedTrigger as ExternalWaitReviewChangedTriggerValue,
	type ExternalWaitTarget as ExternalWaitTargetValue,
	type HostedGitPullRequestLookup as HostedGitPullRequestLookupValue,
} from "@artisan/protocol";

const TerminalCheckState = Schema.Literals([
	"passed",
	"failed",
	"cancelled",
	"skipped",
	"action_required",
	"neutral",
	"timed_out",
]);

type TerminalCheckStateValue = typeof TerminalCheckState.Type;

const CheckEvidence = Schema.Struct({
	name: HostedGitCheck.fields.name,
	origin_native_id: HostedGitOrigin.fields.native_id,
	origin_provider_id: HostedGitOrigin.fields.provider_id,
	origin_resource_kind: HostedGitOrigin.fields.resource_kind,
	required: Schema.Boolean,
	state: HostedGitCheck.fields.state,
	workflow_name: HostedGitCheck.fields.workflow_name,
});
type CheckEvidence = typeof CheckEvidence.Type;

const ReviewEvidence = Schema.Struct({
	origin_native_id: HostedGitOrigin.fields.native_id,
	origin_provider_id: HostedGitOrigin.fields.provider_id,
	state: HostedGitReview.fields.state,
});
type ReviewEvidence = typeof ReviewEvidence.Type;

const ThreadEvidence = Schema.Struct({
	comment_count: HostedGitReviewThread.fields.comment_count,
	last_comment_native_id: HostedGitReviewThread.fields.last_comment_native_id,
	last_updated_at: HostedGitReviewThread.fields.last_updated_at,
	origin_native_id: HostedGitOrigin.fields.native_id,
	origin_provider_id: HostedGitOrigin.fields.provider_id,
	outdated: Schema.Boolean,
	resolved: Schema.Boolean,
});
type ThreadEvidence = typeof ThreadEvidence.Type;

function unique_native_origin_ids<T extends { readonly origin_native_id: string }>(
	evidence: ReadonlyArray<T>,
): boolean {
	return new Set(evidence.map((item) => item.origin_native_id)).size === evidence.length;
}

function check_origin_identity(check: CheckEvidence): string {
	return [check.origin_provider_id, check.origin_resource_kind, check.origin_native_id].join(
		"\u0000",
	);
}

function unique_check_origins(checks: ReadonlyArray<CheckEvidence>): boolean {
	return (
		checks.every(
			(check) =>
				check.origin_resource_kind === "check_run" ||
				check.origin_resource_kind === "status_context",
		) && new Set(checks.map(check_origin_identity)).size === checks.length
	);
}

const CheckEvidenceList = Schema.Array(CheckEvidence).check(
	Schema.makeFilter(unique_check_origins),
);

const ReviewEvidenceList = Schema.Array(ReviewEvidence).check(
	Schema.makeFilter(unique_native_origin_ids),
);

const ThreadEvidenceList = Schema.Array(ThreadEvidence).check(
	Schema.makeFilter(unique_native_origin_ids),
);

const ExternalWaitBaselineSchemaBase = Schema.Struct({
	checks: CheckEvidenceList,
	gates: ExternalWaitRequest.fields.gates,
	pull_request_native_id: HostedGitOrigin.fields.native_id,
	repository: ExternalWaitTarget.fields.repository,
	branch: ExternalWaitTarget.fields.branch,
	expected_head_commit: ExternalWaitTarget.fields.expected_head_commit,
	pull_request_number: ExternalWaitTarget.fields.pull_request_number,
	pull_request_origin: ExternalWaitTarget.fields.pull_request_origin,
	review_decision: HostedGitPullRequest.fields.review_decision,
	reviews: ReviewEvidenceList,
	review_threads: ThreadEvidenceList,
});

const ExternalWaitBaselineSchema = ExternalWaitBaselineSchemaBase.check(
	Schema.makeFilter<typeof ExternalWaitBaselineSchemaBase.Type>((baseline) => {
		const target = {
			branch: baseline.branch,
			expected_head_commit: baseline.expected_head_commit,
			pull_request_number: baseline.pull_request_number,
			pull_request_origin: baseline.pull_request_origin,
			repository: baseline.repository,
		};
		const evidence_providers = [
			...baseline.checks.map((check) => check.origin_provider_id),
			...baseline.reviews.map((review) => review.origin_provider_id),
			...baseline.review_threads.map((thread) => thread.origin_provider_id),
		];

		return Schema.is(ExternalWaitTarget)(target) &&
			baseline.pull_request_native_id === baseline.pull_request_origin.native_id &&
			evidence_providers.every(
				(provider_id) => provider_id === baseline.repository.provider_id,
			)
			? undefined
			: "Expected one internally consistent external-wait target and provider identity";
	}),
);

/** Stores only canonical comparison evidence for one external wait registration. */
export const ExternalWaitBaseline = ExternalWaitBaselineSchema;

export type ExternalWaitBaseline = typeof ExternalWaitBaseline.Type;

const RegistrationInput = Schema.Struct({
	gates: ExternalWaitRequest.fields.gates,
	lookup: HostedGitPullRequestLookup,
	target: ExternalWaitTarget,
});

const EvaluationInput = Schema.Struct({
	baseline: ExternalWaitBaseline,
	lookup: HostedGitPullRequestLookup,
});

/** Identifies why external-wait policy refused to produce a decision. */
export class ExternalWaitPolicyError extends Data.TaggedError("ExternalWaitPolicyError")<{
	readonly reason:
		| "invalid_input"
		| "incomplete_evidence"
		| "identity_mismatch"
		| "unsupported_association"
		| "evidence_bound_exceeded";
}> {}

export type ExternalWaitRegistrationResult =
	| { readonly _tag: "usable"; readonly baseline: ExternalWaitBaseline }
	| { readonly _tag: "already_satisfied" };

export type ExternalWaitEvaluationResult =
	| { readonly _tag: "no_change" }
	| {
			readonly _tag: "wake";
			readonly trigger:
				| ExternalWaitChecksTerminalTriggerValue
				| ExternalWaitReviewChangedTriggerValue;
	  }
	| { readonly _tag: "suspend"; readonly reason: "stale_head" };

function policy_error(reason: ExternalWaitPolicyError["reason"]): ExternalWaitPolicyError {
	return new ExternalWaitPolicyError({ reason });
}

function compare_strings(left: string, right: string): number {
	return left < right ? -1 : left > right ? 1 : 0;
}

function is_terminal(state: string): state is TerminalCheckStateValue {
	return Schema.is(TerminalCheckState)(state);
}

function sort_checks(checks: ReadonlyArray<CheckEvidence>): ReadonlyArray<CheckEvidence> {
	return [...checks].sort(
		(left, right) =>
			compare_strings(left.origin_provider_id, right.origin_provider_id) ||
			compare_strings(left.origin_resource_kind, right.origin_resource_kind) ||
			compare_strings(left.origin_native_id, right.origin_native_id) ||
			compare_strings(left.name, right.name) ||
			compare_strings(left.workflow_name ?? "", right.workflow_name ?? ""),
	);
}

function sort_reviews(reviews: ReadonlyArray<ReviewEvidence>): ReadonlyArray<ReviewEvidence> {
	return [...reviews].sort(
		(left, right) =>
			compare_strings(left.origin_provider_id, right.origin_provider_id) ||
			compare_strings(left.origin_native_id, right.origin_native_id) ||
			compare_strings(left.state, right.state),
	);
}

function sort_threads(threads: ReadonlyArray<ThreadEvidence>): ReadonlyArray<ThreadEvidence> {
	return [...threads].sort(
		(left, right) =>
			compare_strings(left.origin_provider_id, right.origin_provider_id) ||
			compare_strings(left.origin_native_id, right.origin_native_id),
	);
}

function normalize_gate(gate: ExternalWaitGateValue): ExternalWaitGateValue {
	return gate._tag === "selected_checks_terminal"
		? { ...gate, check_names: [...gate.check_names].sort(compare_strings) }
		: gate;
}

function stable_target_matches(
	target: ExternalWaitTargetValue,
	lookup: HostedGitPullRequestLookupValue,
): boolean {
	return (
		lookup.branch === target.branch &&
		lookup.expected_head_commit === target.expected_head_commit &&
		lookup.repository.host === target.repository.host &&
		lookup.repository.name === target.repository.name &&
		lookup.repository.owner === target.repository.owner &&
		lookup.repository.provider_id === target.repository.provider_id &&
		lookup.association._tag === "matched" &&
		lookup.association.pull_request.number === target.pull_request_number &&
		lookup.association.pull_request.origin.native_id === target.pull_request_origin.native_id &&
		lookup.association.pull_request.origin.provider_id === target.repository.provider_id &&
		lookup.association.pull_request.origin.resource_kind === "pull_request" &&
		lookup.association.pull_request.head_branch === target.branch
	);
}

function exact_target_matches(
	target: ExternalWaitTargetValue,
	lookup: HostedGitPullRequestLookupValue,
): boolean {
	return (
		stable_target_matches(target, lookup) &&
		lookup.association._tag === "matched" &&
		lookup.association.pull_request.head_commit === target.expected_head_commit
	);
}

function validate_origins(pull_request: HostedGitPullRequest, provider_id: string): boolean {
	const valid_origin = (
		origin: { readonly provider_id: string; readonly resource_kind: string },
		kinds: ReadonlyArray<string>,
	) => origin.provider_id === provider_id && kinds.includes(origin.resource_kind);

	return (
		valid_origin(pull_request.origin, ["pull_request"]) &&
		unique_check_origins(pull_request.checks.map(check_evidence)) &&
		unique_native_origin_ids(pull_request.reviews.map(review_evidence)) &&
		unique_native_origin_ids(pull_request.review_threads.map(thread_evidence)) &&
		pull_request.reviews.every((review) => valid_origin(review.origin, ["review"])) &&
		pull_request.review_threads.every((thread) =>
			valid_origin(thread.origin, ["review_thread"]),
		) &&
		pull_request.checks.every(
			(check) =>
				valid_origin(check.origin, ["check_run", "status_context"]) &&
				(check.suite_origin === undefined ||
					valid_origin(check.suite_origin, ["check_suite"])) &&
				(check.workflow_origin === undefined ||
					valid_origin(check.workflow_origin, ["workflow_run"])),
		)
	);
}

function complete_collection(length: number, total: number, truncated: boolean): boolean {
	return !truncated && length === total;
}

function check_evidence(check: HostedGitCheck): CheckEvidence {
	return {
		name: check.name,
		origin_native_id: check.origin.native_id,
		origin_provider_id: check.origin.provider_id,
		origin_resource_kind: check.origin.resource_kind,
		required: check.required,
		state: check.state,
		...(check.workflow_name === undefined ? {} : { workflow_name: check.workflow_name }),
	};
}

function review_evidence(review: HostedGitReview): ReviewEvidence {
	return {
		origin_native_id: review.origin.native_id,
		origin_provider_id: review.origin.provider_id,
		state: review.state,
	};
}

function thread_evidence(thread: HostedGitReviewThread): ThreadEvidence {
	return {
		comment_count: thread.comment_count,
		...(thread.last_comment_native_id === undefined
			? {}
			: { last_comment_native_id: thread.last_comment_native_id }),
		...(thread.last_updated_at === undefined
			? {}
			: { last_updated_at: thread.last_updated_at }),
		origin_native_id: thread.origin.native_id,
		origin_provider_id: thread.origin.provider_id,
		outdated: thread.outdated,
		resolved: thread.resolved,
	};
}

function relevant_checks(
	gate: ExternalWaitGateValue,
	checks: ReadonlyArray<CheckEvidence>,
): ReadonlyArray<CheckEvidence> {
	const relevant = Match.value(gate).pipe(
		Match.when({ _tag: "required_checks_terminal" }, () =>
			checks.filter((check) => check.required),
		),
		Match.when({ _tag: "selected_checks_terminal" }, ({ check_names }) =>
			checks.filter((check) => check_names.includes(check.name)),
		),
		Match.orElse(() => []),
	);

	return sort_checks(relevant);
}

function reviews_complete(pr: HostedGitPullRequest): boolean {
	return complete_collection(pr.reviews.length, pr.reviews_total, pr.reviews_truncated);
}

function threads_complete(pr: HostedGitPullRequest): boolean {
	return complete_collection(
		pr.review_threads.length,
		pr.review_threads_total,
		pr.review_threads_truncated,
	);
}

function checks_complete(pr: HostedGitPullRequest): boolean {
	return complete_collection(pr.checks.length, pr.checks_total, pr.checks_truncated);
}

function canonical_baseline(
	target: ExternalWaitTargetValue,
	gates: ReadonlyArray<ExternalWaitGateValue>,
	pr: HostedGitPullRequest,
): ExternalWaitBaseline {
	const check_requested = gates.some(
		(gate) =>
			gate._tag === "required_checks_terminal" || gate._tag === "selected_checks_terminal",
	);
	const review_requested = gates.some((gate) => gate._tag === "review_submitted");
	const thread_requested = gates.some((gate) => gate._tag === "review_threads_changed");
	const checks = pr.checks.map(check_evidence);

	return {
		branch: target.branch,
		checks: check_requested
			? sort_checks(
					checks.filter((check) =>
						gates.some((gate) => relevant_checks(gate, [check]).length > 0),
					),
				)
			: [],
		expected_head_commit: target.expected_head_commit,
		gates: gates
			.map(normalize_gate)
			.sort((left, right) => compare_strings(left._tag, right._tag)),
		pull_request_native_id: target.pull_request_origin.native_id,
		pull_request_number: target.pull_request_number,
		pull_request_origin: target.pull_request_origin,
		repository: target.repository,
		review_decision: pr.review_decision,
		review_threads: thread_requested
			? sort_threads(pr.review_threads.map(thread_evidence))
			: [],
		reviews: review_requested ? sort_reviews(pr.reviews.map(review_evidence)) : [],
	};
}

function check_gate_already_satisfied(
	gate: ExternalWaitGateValue,
	checks: ReadonlyArray<CheckEvidence>,
): boolean {
	const relevant = relevant_checks(gate, checks);
	if (gate._tag === "required_checks_terminal") {
		return relevant.every((check) => is_terminal(check.state));
	}

	const selected_names = new Set(relevant.map((check) => check.name));
	const selected_complete =
		gate._tag === "selected_checks_terminal" &&
		gate.check_names.every((name) => selected_names.has(name));

	return (
		gate._tag === "selected_checks_terminal" &&
		selected_complete &&
		relevant.every((check) => is_terminal(check.state))
	);
}

function validate_registration_evidence(
	gates: ReadonlyArray<ExternalWaitGateValue>,
	pr: HostedGitPullRequest,
): ExternalWaitPolicyError | undefined {
	const check_requested = gates.some(
		(gate) =>
			gate._tag === "required_checks_terminal" || gate._tag === "selected_checks_terminal",
	);
	const review_requested = gates.some((gate) => gate._tag === "review_submitted");
	const thread_requested = gates.some((gate) => gate._tag === "review_threads_changed");

	if (
		(check_requested && !checks_complete(pr)) ||
		(review_requested && !reviews_complete(pr)) ||
		(thread_requested && !threads_complete(pr))
	) {
		return policy_error("incomplete_evidence");
	}

	const checks = pr.checks.map(check_evidence);
	if (gates.some((gate) => relevant_checks(gate, checks).length > 64)) {
		return policy_error("evidence_bound_exceeded");
	}

	return undefined;
}

function validate_evaluation_evidence(
	gates: ReadonlyArray<ExternalWaitGateValue>,
	pr: HostedGitPullRequest,
): ExternalWaitPolicyError | undefined {
	const check_requested = gates.some(
		(gate) =>
			gate._tag === "required_checks_terminal" || gate._tag === "selected_checks_terminal",
	);
	const review_requested = gates.some((gate) => gate._tag === "review_submitted");
	const review_trigger_requested = gates.some(
		(gate) =>
			gate._tag === "review_decision_changed" ||
			gate._tag === "review_submitted" ||
			gate._tag === "review_threads_changed",
	);

	if (
		(check_requested && !checks_complete(pr)) ||
		(review_requested && !reviews_complete(pr)) ||
		(review_trigger_requested && !threads_complete(pr))
	) {
		return policy_error("incomplete_evidence");
	}

	const checks = pr.checks.map(check_evidence);

	return gates.some((gate) => relevant_checks(gate, checks).length > 64)
		? policy_error("evidence_bound_exceeded")
		: undefined;
}

function checks_trigger(
	summaries: ReadonlyArray<CheckEvidence>,
): Effect.Effect<ExternalWaitEvaluationResult, ExternalWaitPolicyError> {
	if (
		summaries.length === 0 ||
		summaries.length > 64 ||
		summaries.some((summary) => !is_terminal(summary.state))
	) {
		return Effect.fail(policy_error("evidence_bound_exceeded"));
	}

	const terminal_summaries = summaries.filter(
		(summary): summary is CheckEvidence & { readonly state: TerminalCheckStateValue } =>
			is_terminal(summary.state),
	);
	const sorted_terminal_summaries = [...terminal_summaries].sort(
		(left, right) =>
			compare_strings(left.origin_provider_id, right.origin_provider_id) ||
			compare_strings(left.origin_resource_kind, right.origin_resource_kind) ||
			compare_strings(left.origin_native_id, right.origin_native_id) ||
			compare_strings(left.name, right.name) ||
			compare_strings(left.workflow_name ?? "", right.workflow_name ?? ""),
	);
	const check_summaries: ReadonlyArray<ExternalWaitCheckSummary> = sorted_terminal_summaries.map(
		(summary) => ({
			name: summary.name,
			required: summary.required,
			state: summary.state,
			...(summary.workflow_name === undefined
				? {}
				: { workflow_name: summary.workflow_name }),
		}),
	);

	return Schema.decodeUnknownEffect(ExternalWaitChecksTerminalTrigger)({
		_tag: "checks_terminal",
		check_summaries,
		truncated: false,
	}).pipe(
		Effect.mapError(() => policy_error("invalid_input")),
		Effect.map((trigger) => ({ _tag: "wake" as const, trigger })),
	);
}

/** Canonically serializes a validated baseline without provider text or array-order variance. */
export function serialize_external_wait_baseline(baseline: ExternalWaitBaseline): string {
	return JSON.stringify({
		...baseline,
		checks: sort_checks(baseline.checks),
		gates: baseline.gates
			.map(normalize_gate)
			.sort((left, right) => compare_strings(left._tag, right._tag)),
		reviews: sort_reviews(baseline.reviews),
		review_threads: sort_threads(baseline.review_threads),
	});
}

/** Validates a registration lookup and captures its provider-neutral comparison baseline. */
export const BuildExternalWaitBaseline = (
	input: unknown,
): Effect.Effect<ExternalWaitRegistrationResult, ExternalWaitPolicyError> =>
	Schema.decodeUnknownEffect(RegistrationInput, { onExcessProperty: "error" })(input).pipe(
		Effect.mapError(() => policy_error("invalid_input")),
		Effect.flatMap(({ gates, lookup, target }) => {
			if (lookup.association._tag !== "matched") {
				return Effect.fail(policy_error("unsupported_association"));
			}

			const pr = lookup.association.pull_request;
			if (
				!exact_target_matches(target, lookup) ||
				lookup.association.freshness !== "current" ||
				!validate_origins(pr, target.repository.provider_id)
			) {
				return Effect.fail(policy_error("identity_mismatch"));
			}

			const evidence_error = validate_registration_evidence(gates, pr);
			if (evidence_error !== undefined) {
				return Effect.fail(evidence_error);
			}

			const checks = pr.checks.map(check_evidence);
			if (gates.some((gate) => check_gate_already_satisfied(gate, checks))) {
				return Effect.succeed<ExternalWaitRegistrationResult>({
					_tag: "already_satisfied",
				});
			}

			return Effect.succeed<ExternalWaitRegistrationResult>({
				_tag: "usable",
				baseline: canonical_baseline(target, gates, pr),
			});
		}),
	);

/** Evaluates a later exact-head lookup against a validated registration baseline. */
export const EvaluateExternalWait = (
	input: unknown,
): Effect.Effect<ExternalWaitEvaluationResult, ExternalWaitPolicyError> =>
	Schema.decodeUnknownEffect(EvaluationInput, { onExcessProperty: "error" })(input).pipe(
		Effect.mapError(() => policy_error("invalid_input")),
		Effect.flatMap(
			({
				baseline,
				lookup,
			}: {
				readonly baseline: ExternalWaitBaseline;
				readonly lookup: HostedGitPullRequestLookupValue;
			}): Effect.Effect<ExternalWaitEvaluationResult, ExternalWaitPolicyError> => {
				if (lookup.association._tag !== "matched") {
					return Effect.fail(policy_error("unsupported_association"));
				}

				const pr = lookup.association.pull_request;
				const baseline_target = {
					branch: baseline.branch,
					expected_head_commit: baseline.expected_head_commit,
					pull_request_number: baseline.pull_request_number,
					pull_request_origin: baseline.pull_request_origin,
					repository: baseline.repository,
				};

				if (
					!stable_target_matches(baseline_target, lookup) ||
					!validate_origins(pr, baseline.repository.provider_id)
				) {
					return Effect.fail(policy_error("identity_mismatch"));
				}

				if (lookup.association.freshness === "stale_head") {
					return Effect.succeed<ExternalWaitEvaluationResult>({
						_tag: "suspend",
						reason: "stale_head",
					});
				}

				if (!exact_target_matches(baseline_target, lookup)) {
					return Effect.fail(policy_error("identity_mismatch"));
				}

				const evidence_error = validate_evaluation_evidence(baseline.gates, pr);
				if (evidence_error !== undefined) {
					return Effect.fail(evidence_error);
				}

				const current_checks = pr.checks.map(check_evidence);
				const current_reviews = sort_reviews(pr.reviews.map(review_evidence));
				const current_threads = sort_threads(pr.review_threads.map(thread_evidence));

				for (const gate of baseline.gates) {
					if (
						gate._tag === "required_checks_terminal" ||
						gate._tag === "selected_checks_terminal"
					) {
						const previous = relevant_checks(gate, baseline.checks);
						const current = relevant_checks(gate, current_checks);
						const selected_names = new Set(current.map((check) => check.name));
						const selected_complete =
							gate._tag === "required_checks_terminal" ||
							gate.check_names.every((name) => selected_names.has(name));

						if (
							selected_complete &&
							current.length > 0 &&
							current.every((check) => is_terminal(check.state)) &&
							!check_gate_already_satisfied(gate, previous)
						) {
							return checks_trigger(current);
						}
					}

					if (
						gate._tag === "review_decision_changed" &&
						pr.review_decision !== baseline.review_decision
					) {
						return Schema.decodeUnknownEffect(ExternalWaitReviewChangedTrigger)({
							_tag: "review_changed",
							change_kind: "decision_changed",
							decision: pr.review_decision,
							total_reviews: pr.reviews_total,
							unresolved_thread_count: pr.review_threads.filter(
								(thread) => !thread.resolved,
							).length,
						}).pipe(
							Effect.mapError(() => policy_error("invalid_input")),
							Effect.map((trigger) => ({ _tag: "wake" as const, trigger })),
						);
					}

					if (
						gate._tag === "review_submitted" &&
						current_reviews.some(
							(review) =>
								!baseline.reviews.some(
									(previous) =>
										previous.origin_native_id === review.origin_native_id,
								),
						)
					) {
						return Schema.decodeUnknownEffect(ExternalWaitReviewChangedTrigger)({
							_tag: "review_changed",
							change_kind: "review_submitted",
							decision: pr.review_decision,
							total_reviews: pr.reviews_total,
							unresolved_thread_count: pr.review_threads.filter(
								(thread) => !thread.resolved,
							).length,
						}).pipe(
							Effect.mapError(() => policy_error("invalid_input")),
							Effect.map((trigger) => ({ _tag: "wake" as const, trigger })),
						);
					}

					if (
						gate._tag === "review_threads_changed" &&
						JSON.stringify(current_threads) !== JSON.stringify(baseline.review_threads)
					) {
						return Schema.decodeUnknownEffect(ExternalWaitReviewChangedTrigger)({
							_tag: "review_changed",
							change_kind: "threads_changed",
							decision: pr.review_decision,
							total_reviews: pr.reviews_total,
							unresolved_thread_count: pr.review_threads.filter(
								(thread) => !thread.resolved,
							).length,
						}).pipe(
							Effect.mapError(() => policy_error("invalid_input")),
							Effect.map((trigger) => ({ _tag: "wake" as const, trigger })),
						);
					}
				}

				return Effect.succeed<ExternalWaitEvaluationResult>({ _tag: "no_change" });
			},
		),
	);
