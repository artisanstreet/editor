import { Buffer } from "node:buffer";

import { Cause, Data, Effect, Layer, Schema } from "effect";
import { HostedGitPullRequestLookup, type HostedGitRequestedReviewer } from "@artisan/protocol";

import {
	GitProvider,
	GitProviderCloneExecution,
	GitProviderClonePreparation,
	GitProviderCloneRequest,
	GitProviderCloneResult,
	GitProviderDiscovery,
	GitProviderDiscoveryScope,
	GitProviderError,
	GitProviderInspection,
	GitProviderPage,
	GitProviderPullRequestRead,
	GitProviderPullRequestTargetRead,
	normalize_git_provider_host,
	type GitProviderAccountAuthentication,
	type GitProviderCloneExecution as GitProviderCloneExecutionInput,
	type GitProviderClonePreparation as GitProviderClonePreparationResult,
	type GitProviderCloneRequest as GitProviderCloneRequestInput,
	type GitProviderDiscovery as GitProviderDiscoveryInput,
	type GitProviderDiscoveryScope as GitProviderDiscoveryScopeInput,
	type GitProviderHostAuthentication,
	type GitProviderInspection as GitProviderInspectionResult,
	type GitProviderPage as GitProviderPageResult,
	type GitProviderPullRequestRead as GitProviderPullRequestReadInput,
	type GitProviderPullRequestTargetRead as GitProviderPullRequestTargetReadInput,
} from "../git-provider";
import {
	GitHubCli,
	GitHubCliError,
	github_https_clone_url,
	type GitHubCliAccount,
	type GitHubCliInspection,
	type GitHubCliPullRequestReadResult,
	type GitHubCliRepository,
} from "./github-cli";

const GitHubCursorPayload = Schema.Struct({
	account_login: Schema.NonEmptyString,
	host: Schema.NonEmptyString,
	native_cursor: Schema.NonEmptyString,
	page_size: Schema.Int,
	provider_id: Schema.Literal("github"),
	scope: GitProviderDiscoveryScope,
	version: Schema.Literal(1),
});

const github_provider_id = "github" as const;

/** Rejects invalid statically configured GitHub hosts. */
export class GitHubProviderConfigurationError extends Data.TaggedError(
	"GitHubProviderConfigurationError",
)<{
	readonly reason: "duplicate_host" | "invalid_host";
}> {}

/** Configures exact GitHub hosts that resolve even before an account is authenticated. */
export interface GitHubProviderOptions {
	readonly hosts?: ReadonlyArray<string>;
}

function provider_operation_error(
	operation: GitProviderError["operation"],
	reason: GitProviderError["reason"],
	retryable: boolean,
	host?: string,
) {
	return new GitProviderError({
		...(host === undefined ? {} : { host }),
		operation,
		provider_id: github_provider_id,
		reason,
		retryable,
	});
}

function provider_error(reason: GitProviderError["reason"], retryable: boolean, host?: string) {
	return provider_operation_error("discover_repositories", reason, retryable, host);
}

function inspect_error() {
	return new GitProviderError({
		operation: "inspect",
		provider_id: github_provider_id,
		reason: "invalid_response",
		retryable: false,
	});
}

function installation_reason(inspection: Extract<GitHubCliInspection, { type: "unavailable" }>) {
	const reasons = {
		auth_probe_failed: "GitHub CLI authentication inspection failed",
		invalid_output: "GitHub CLI returned an invalid inspection response",
		process_failed: "GitHub CLI could not be executed",
		timed_out: "GitHub CLI authentication inspection timed out",
	} as const;

	return reasons[inspection.reason];
}

function account_authentication(account: GitHubCliAccount): GitProviderAccountAuthentication {
	return account.type === "authenticated"
		? {
				_tag: "authenticated",
				account_login: account.login,
			}
		: {
				_tag: "authentication_required",
				...(account.login === undefined ? {} : { account_login: account.login }),
			};
}

function MapHostAuthentication(host: string, accounts: ReadonlyArray<GitHubCliAccount>) {
	return Effect.gen(function* () {
		const normalized_host = normalize_git_provider_host(host);
		const normalized_accounts = accounts.map(account_authentication);
		const account_logins = normalized_accounts.flatMap((account) =>
			account.account_login === undefined ? [] : [account.account_login.toLowerCase()],
		);
		const active = accounts.filter((account) => account.active);

		if (
			normalized_host === undefined ||
			new Set(account_logins).size !== account_logins.length ||
			active.length > 1
		) {
			return yield* Effect.fail(inspect_error());
		}

		const active_account = active[0];
		const selected =
			active_account?.type === "authenticated"
				? {
						_tag: "selected" as const,
						account_login: active_account.login,
					}
				: ({ _tag: "none" } as const);

		return {
			accounts: normalized_accounts,
			active_account: selected,
			host: normalized_host,
		} satisfies GitProviderHostAuthentication;
	});
}

function ValidateInspection(value: GitProviderInspectionResult) {
	return Schema.decodeUnknownEffect(GitProviderInspection, {
		onExcessProperty: "error",
	})(value).pipe(Effect.mapError(() => inspect_error()));
}

function MapInspection(inspection: GitHubCliInspection, configured_hosts: ReadonlyArray<string>) {
	if (inspection.type === "missing") {
		return ValidateInspection({
			authentication: configured_hosts.map((host) => ({
				accounts: [],
				active_account: { _tag: "none" },
				host,
			})),
			installation: { _tag: "missing" },
		});
	}

	if (inspection.type === "incompatible") {
		return ValidateInspection({
			authentication: configured_hosts.map((host) => ({
				accounts: [],
				active_account: { _tag: "none" },
				host,
			})),
			installation: {
				_tag: "incompatible",
				executable_path: inspection.executable_path,
				installed_version: inspection.version,
				reason: "Required GitHub CLI JSON authentication features are unavailable",
			},
		});
	}

	if (inspection.type === "unavailable") {
		return ValidateInspection({
			authentication: configured_hosts.map((host) => ({
				accounts: [],
				active_account: { _tag: "none" },
				host,
			})),
			installation: {
				_tag: "unavailable",
				...(inspection.executable_path === undefined
					? {}
					: { executable_path: inspection.executable_path }),
				reason: installation_reason(inspection),
				...(inspection.version === undefined ? {} : { version: inspection.version }),
			},
		});
	}

	return Effect.gen(function* () {
		const mapped = yield* Effect.forEach(inspection.hosts, (authentication) =>
			MapHostAuthentication(authentication.host, authentication.accounts),
		);

		if (new Set(mapped.map((authentication) => authentication.host)).size !== mapped.length) {
			return yield* Effect.fail(inspect_error());
		}

		const by_host = new Map(
			mapped.map((authentication) => [authentication.host, authentication]),
		);

		for (const host of configured_hosts) {
			if (!by_host.has(host)) {
				by_host.set(host, {
					accounts: [],
					active_account: { _tag: "none" },
					host,
				});
			}
		}

		return yield* ValidateInspection({
			authentication: [...by_host.values()].toSorted((left, right) =>
				left.host.localeCompare(right.host),
			),
			installation: {
				_tag: "available",
				executable_path: inspection.executable_path,
				version: inspection.version,
			},
		});
	});
}

function cli_error(
	cause: GitHubCliError,
	host: string,
	operation: GitProviderError["operation"] = "discover_repositories",
) {
	const reasons: Readonly<Record<GitHubCliError["reason"], GitProviderError["reason"]>> = {
		authentication_required: "auth_required",
		dependency_incompatible: "cli_incompatible",
		dependency_missing: "cli_missing",
		git_dependency_missing: "git_missing",
		invalid_response: "invalid_response",
		invalid_destination: "invalid_input",
		network_unavailable: "network",
		outcome_unknown: "outcome_unknown",
		permission_insufficient: "permission_denied",
		process_failed: "cli_unavailable",
		rate_limited: "rate_limited",
		remote_not_found: "not_found",
		remote_rejected: "remote_rejected",
		timed_out: "timed_out",
	};

	const reason =
		operation === "clone_repository" && cause.reason === "remote_rejected"
			? "clone_failed"
			: reasons[cause.reason];

	return provider_operation_error(operation, reason, cause.retryable, host);
}

function installation_error(
	installation: GitProviderInspectionResult["installation"],
	host: string,
	operation: GitProviderError["operation"] = "discover_repositories",
) {
	if (installation._tag === "missing") {
		return provider_operation_error(operation, "cli_missing", false, host);
	}

	if (installation._tag === "incompatible") {
		return provider_operation_error(operation, "cli_incompatible", false, host);
	}

	return installation._tag === "unavailable"
		? provider_operation_error(operation, "cli_unavailable", true, host)
		: undefined;
}

function cursor_scope_identity(scope: GitProviderDiscoveryScopeInput) {
	if (scope._tag === "organization") {
		return `organization:${scope.organization}`;
	}

	return scope._tag === "search" ? `search:${scope.query}` : "account";
}

function github_login_key(value: string) {
	return value.toLowerCase();
}

function github_logins_match(left: string, right: string) {
	return github_login_key(left) === github_login_key(right);
}

function encode_cursor(input: {
	readonly account_login: string;
	readonly host: string;
	readonly native_cursor: string;
	readonly page_size: number;
	readonly scope: GitProviderDiscoveryScopeInput;
}) {
	return Buffer.from(
		JSON.stringify({
			...input,
			account_login: github_login_key(input.account_login),
			provider_id: github_provider_id,
			version: 1,
		}),
	).toString("base64url");
}

function DecodeCursor(input: GitProviderDiscoveryInput) {
	if (input.position._tag === "first") {
		return Effect.succeed(undefined);
	}

	if (!/^[A-Za-z0-9_-]+$/u.test(input.position.cursor)) {
		return Effect.fail(provider_error("invalid_cursor", false, input.selection.host));
	}

	const encoded_cursor = input.position.cursor;

	return Effect.try(() =>
		JSON.parse(Buffer.from(encoded_cursor, "base64url").toString("utf8")),
	).pipe(
		Effect.flatMap((value) =>
			Schema.decodeUnknownEffect(GitHubCursorPayload, {
				onExcessProperty: "error",
			})(value),
		),
		Effect.mapError(() => provider_error("invalid_cursor", false, input.selection.host)),
		Effect.flatMap((cursor) =>
			cursor.provider_id !== input.selection.provider_id ||
			cursor.host !== input.selection.host ||
			!github_logins_match(cursor.account_login, input.selection.account_login) ||
			cursor.page_size !== input.page_size ||
			cursor_scope_identity(cursor.scope) !== cursor_scope_identity(input.scope)
				? Effect.fail(provider_error("invalid_cursor", false, input.selection.host))
				: Effect.succeed(cursor.native_cursor),
		),
	);
}

function clone_url_hostname(value: string) {
	if (URL.canParse(value)) {
		return URL.parse(value)?.hostname;
	}

	const scp_host = /^[A-Za-z0-9._-]+@(?<host>[A-Za-z0-9.-]+):/u.exec(value)?.groups?.host;

	return scp_host === undefined ? undefined : normalize_git_provider_host(scp_host);
}

function repository_urls_match_host(host: string, repository: GitHubCliRepository) {
	const selected = URL.parse(`https://${host}`);
	const web_url = URL.parse(repository.web_url);
	const web_host = web_url === null ? undefined : normalize_git_provider_host(web_url.host);

	return (
		selected !== null &&
		clone_url_hostname(repository.ssh_url) === selected.hostname &&
		web_host === host
	);
}

function map_repository(
	host: string,
	repository: GitHubCliRepository,
	operation: GitProviderError["operation"] = "discover_repositories",
) {
	if (
		repository.name_with_owner !== `${repository.owner}/${repository.name}` ||
		!repository_urls_match_host(host, repository)
	) {
		return Effect.fail(provider_operation_error(operation, "invalid_response", false, host));
	}

	return Effect.succeed({
		archived: repository.archived,
		clone_url: github_https_clone_url({
			host,
			name: repository.name,
			owner: repository.owner,
		}),
		default_branch:
			repository.default_branch === undefined
				? ({ _tag: "unavailable" } as const)
				: ({ _tag: "known", name: repository.default_branch } as const),
		identity: {
			host,
			name: repository.name,
			owner: repository.owner,
			provider_id: github_provider_id,
		},
		origin: {
			native_id: repository.native_id,
			provider_id: github_provider_id,
			resource_kind: "repository" as const,
		},
		viewer_permission: repository.viewer_permission,
		visibility: repository.visibility,
		web_url: repository.web_url,
	});
}

function ValidatePage(value: GitProviderPageResult, host: string) {
	return Schema.decodeUnknownEffect(GitProviderPage, {
		onExcessProperty: "error",
	})(value).pipe(Effect.mapError(() => provider_error("invalid_response", false, host)));
}

function ValidatePullRequest(
	value: unknown,
	host: string,
	operation: GitProviderError["operation"] = "read_pull_request",
) {
	return Schema.decodeUnknownEffect(HostedGitPullRequestLookup, {
		onExcessProperty: "error",
	})(value).pipe(
		Effect.mapError(() => provider_operation_error(operation, "invalid_response", false, host)),
	);
}

function pull_request_state(state: string, merged: boolean) {
	if (merged) {
		return "merged" as const;
	}

	return state === "OPEN" ? ("open" as const) : ("closed" as const);
}

type GitHubMatchedPullRequest = Extract<
	GitHubCliPullRequestReadResult,
	{ readonly type: "matched_pull_request" }
>["pull_request"];

function review_state(state: GitHubMatchedPullRequest["reviews"]["nodes"][number]["state"]) {
	const states = {
		APPROVED: "approved",
		CHANGES_REQUESTED: "changes_requested",
		COMMENTED: "commented",
		DISMISSED: "dismissed",
	} as const;

	return states[state];
}

function mergeability(value: GitHubMatchedPullRequest["mergeable"]) {
	const values = {
		CONFLICTING: "conflicting",
		MERGEABLE: "mergeable",
		UNKNOWN: "unknown",
	} as const;

	return values[value];
}

function review_decision(value: GitHubMatchedPullRequest["reviewDecision"]) {
	const values = {
		APPROVED: "approved",
		CHANGES_REQUESTED: "changes_requested",
		REVIEW_REQUIRED: "review_required",
	} as const;

	return value === null ? ("none" as const) : values[value];
}

function annotation_level(value: "NOTICE" | "WARNING" | "FAILURE") {
	const levels = {
		FAILURE: "failure",
		NOTICE: "notice",
		WARNING: "warning",
	} as const;

	return levels[value];
}

function map_requested_reviewer(
	request: GitHubMatchedPullRequest["requestedReviewers"]["nodes"][number],
): ReadonlyArray<HostedGitRequestedReviewer> {
	const reviewer = request.requestedReviewer;

	if (reviewer === null) {
		return [];
	}

	if (reviewer.__typename === "User") {
		return [{ _tag: "user", login: reviewer.login }];
	}

	if (reviewer.__typename === "Team") {
		return [{ _tag: "team", organization: reviewer.organization.login, slug: reviewer.slug }];
	}

	return [];
}

function check_run_state(
	check: Extract<
		GitHubMatchedPullRequest["commits"]["nodes"][number]["commit"]["statusCheckRollup"],
		object
	>["contexts"]["nodes"][number],
) {
	if (check.__typename === "StatusContext") {
		const states: Readonly<
			Record<string, "queued" | "running" | "passed" | "failed" | "unknown">
		> = {
			ERROR: "failed",
			EXPECTED: "queued",
			FAILURE: "failed",
			PENDING: "running",
			SUCCESS: "passed",
		};

		return states[check.state] ?? "unknown";
	}

	if (check.conclusion !== null) {
		const conclusions: Readonly<
			Record<
				string,
				| "action_required"
				| "cancelled"
				| "failed"
				| "neutral"
				| "passed"
				| "skipped"
				| "stale"
				| "timed_out"
				| "unknown"
			>
		> = {
			ACTION_REQUIRED: "action_required",
			CANCELLED: "cancelled",
			FAILURE: "failed",
			NEUTRAL: "neutral",
			SKIPPED: "skipped",
			STALE: "stale",
			STARTUP_FAILURE: "failed",
			SUCCESS: "passed",
			TIMED_OUT: "timed_out",
		};

		return conclusions[check.conclusion] ?? "unknown";
	}

	const statuses: Readonly<Record<string, "queued" | "running" | "unknown">> = {
		IN_PROGRESS: "running",
		PENDING: "queued",
		QUEUED: "queued",
		REQUESTED: "queued",
		WAITING: "queued",
	};

	return statuses[check.status] ?? "unknown";
}

function MapCheck(
	check: Extract<
		GitHubMatchedPullRequest["commits"]["nodes"][number]["commit"]["statusCheckRollup"],
		object
	>["contexts"]["nodes"][number],
) {
	if (check.__typename === "StatusContext") {
		return {
			annotations: [],
			annotations_truncated: false,
			...(check.targetUrl === null ? {} : { details_url: check.targetUrl }),
			name: check.context,
			origin: {
				native_id: check.id,
				provider_id: github_provider_id,
				resource_kind: "status_context" as const,
			},
			required: check.isRequired,
			state: check_run_state(check),
		};
	}

	const workflow_run = check.checkSuite.workflowRun;
	const annotations = check.annotations;

	return {
		annotations: (annotations?.nodes ?? []).flatMap((annotation) =>
			annotation.annotationLevel === null
				? []
				: [
						{
							end_line: annotation.location.end.line,
							level: annotation_level(annotation.annotationLevel),
							path: annotation.path,
							start_line: annotation.location.start.line,
							...(annotation.title === null ? {} : { title: annotation.title }),
							untrusted_message: annotation.message,
						},
					],
		),
		annotations_truncated:
			annotations === null ||
			annotations.pageInfo.hasNextPage ||
			annotations.nodes.some((annotation) => annotation.annotationLevel === null),
		...(check.checkSuite.app === null ? {} : { app_name: check.checkSuite.app.name }),
		...(check.completedAt === null ? {} : { completed_at: check.completedAt }),
		...(check.detailsUrl === null ? {} : { details_url: check.detailsUrl }),
		name: check.name,
		origin: {
			native_id: check.id,
			provider_id: github_provider_id,
			resource_kind: "check_run" as const,
		},
		required: check.isRequired,
		...(check.startedAt === null ? {} : { started_at: check.startedAt }),
		state: check_run_state(check),
		suite_origin: {
			native_id: check.checkSuite.id,
			provider_id: github_provider_id,
			resource_kind: "check_suite" as const,
		},
		...(workflow_run === null
			? {}
			: {
					attempt: workflow_run.runAttempt,
					workflow_name: workflow_run.workflow.name,
					workflow_origin: {
						native_id: workflow_run.id,
						provider_id: github_provider_id,
						resource_kind: "workflow_run" as const,
					},
					workflow_url: workflow_run.url,
				}),
	};
}

function MapMatchedPullRequest(
	input: GitProviderPullRequestReadInput,
	pull_request: GitHubMatchedPullRequest,
) {
	const rollup = pull_request.commits.nodes[0]?.commit.statusCheckRollup;
	const checks = rollup?.contexts.nodes ?? [];

	return {
		association: {
			_tag: "matched" as const,
			freshness:
				pull_request.headRefOid === input.expected_head
					? ("current" as const)
					: ("stale_head" as const),
			pull_request: {
				base_branch: pull_request.baseRefName,
				base_commit: pull_request.baseRefOid,
				checks: checks.map(MapCheck),
				checks_total: rollup?.contexts.totalCount ?? 0,
				checks_truncated: rollup?.contexts.pageInfo.hasNextPage ?? false,
				draft: pull_request.isDraft,
				head_branch: pull_request.headRefName,
				head_commit: pull_request.headRefOid,
				mergeability: mergeability(pull_request.mergeable),
				number: pull_request.number,
				origin: {
					native_id: pull_request.id,
					provider_id: github_provider_id,
					resource_kind: "pull_request" as const,
				},
				requested_reviewers:
					pull_request.requestedReviewers.nodes.flatMap(map_requested_reviewer),
				requested_reviewers_truncated:
					pull_request.requestedReviewers.pageInfo.hasNextPage ||
					pull_request.requestedReviewers.nodes.some(
						(request) =>
							request.requestedReviewer?.__typename !== "User" &&
							request.requestedReviewer?.__typename !== "Team",
					),
				review_decision: review_decision(pull_request.reviewDecision),
				review_threads: pull_request.reviewThreads.nodes.map((thread) => {
					const last_comment = thread.comments.nodes[0];

					return {
						comment_count: thread.comments.totalCount,
						...(last_comment === undefined
							? {}
							: {
									last_comment_native_id: last_comment.id,
									last_updated_at: last_comment.updatedAt,
								}),
						...(thread.line === null ? {} : { line: thread.line }),
						origin: {
							native_id: thread.id,
							provider_id: github_provider_id,
							resource_kind: "review_thread" as const,
						},
						outdated: thread.isOutdated,
						path: thread.path,
						resolved: thread.isResolved,
						subject:
							thread.subjectType === "FILE" ? ("file" as const) : ("line" as const),
					};
				}),
				review_threads_total: pull_request.reviewThreads.totalCount,
				review_threads_truncated: pull_request.reviewThreads.pageInfo.hasNextPage,
				reviews: pull_request.reviews.nodes.map((review) => ({
					...(review.author === null ? {} : { author: review.author.login }),
					...(review.commit === null ? {} : { commit: review.commit.oid }),
					origin: {
						native_id: review.id,
						provider_id: github_provider_id,
						resource_kind: "review" as const,
					},
					state: review_state(review.state),
					submitted_at: review.submittedAt,
				})),
				reviews_total: pull_request.reviews.totalCount,
				reviews_truncated: pull_request.reviews.pageInfo.hasNextPage,
				state: pull_request_state(pull_request.state, pull_request.isMerged),
				title: pull_request.title,
				web_url: pull_request.url,
			},
		},
		branch: input.selected_branch,
		expected_head_commit: input.expected_head,
		repository: input.repository,
	};
}

function repositories_match(
	left: GitProviderClonePreparationResult["repository"],
	right: GitProviderClonePreparationResult["repository"],
) {
	return (
		left.identity.host === right.identity.host &&
		left.identity.name === right.identity.name &&
		left.identity.owner === right.identity.owner &&
		left.identity.provider_id === right.identity.provider_id &&
		left.origin.native_id === right.origin.native_id &&
		left.origin.provider_id === right.origin.provider_id
	);
}

/** Builds the sole live V1 hosted-Git adapter over an authenticated GitHub CLI session. */
export function make_github_provider_layer(options: GitHubProviderOptions = {}) {
	const hosts = ["github.com", ...(options.hosts ?? [])].map(normalize_git_provider_host);

	if (hosts.some((host) => host === undefined)) {
		return Layer.effect(
			GitProvider,
			Effect.fail(new GitHubProviderConfigurationError({ reason: "invalid_host" })),
		);
	}

	const configured_hosts = hosts.filter((host): host is string => host !== undefined);

	if (new Set(configured_hosts).size !== configured_hosts.length) {
		return Layer.effect(
			GitProvider,
			Effect.fail(new GitHubProviderConfigurationError({ reason: "duplicate_host" })),
		);
	}

	return Layer.effect(
		GitProvider,
		Effect.gen(function* () {
			const cli = yield* GitHubCli;
			const Inspect = cli.Inspect.pipe(
				Effect.flatMap((inspection) => MapInspection(inspection, configured_hosts)),
			);
			const EnsureSelection = (
				selection: GitProviderClonePreparationResult["selection"],
				operation: GitProviderError["operation"],
			) =>
				Effect.gen(function* () {
					if (selection.provider_id !== github_provider_id) {
						return yield* Effect.fail(
							provider_operation_error(operation, "invalid_input", false),
						);
					}

					const inspection = yield* Inspect;
					const unavailable = installation_error(
						inspection.installation,
						selection.host,
						operation,
					);

					if (unavailable !== undefined) {
						return yield* Effect.fail(unavailable);
					}

					const authentication = inspection.authentication.find(
						(candidate) => candidate.host === selection.host,
					);

					if (authentication === undefined) {
						return yield* Effect.fail(
							provider_operation_error(
								operation,
								"unsupported_host",
								false,
								selection.host,
							),
						);
					}

					if (authentication.active_account._tag === "none") {
						return yield* Effect.fail(
							provider_operation_error(
								operation,
								"auth_required",
								false,
								selection.host,
							),
						);
					}

					if (
						!github_logins_match(
							authentication.active_account.account_login,
							selection.account_login,
						)
					) {
						return yield* Effect.fail(
							provider_operation_error(
								operation,
								"account_not_active",
								false,
								selection.host,
							),
						);
					}
				});
			const DiscoverRepositories = (unknown_input: GitProviderDiscoveryInput) =>
				Effect.gen(function* () {
					const input = yield* Schema.decodeUnknownEffect(GitProviderDiscovery, {
						onExcessProperty: "error",
					})(unknown_input).pipe(
						Effect.mapError(() => provider_error("invalid_input", false)),
					);

					yield* EnsureSelection(input.selection, "discover_repositories");

					const native_cursor = yield* DecodeCursor(input);
					const page = yield* cli
						.QueryRepositories({
							host: input.selection.host,
							...(native_cursor === undefined ? {} : { native_cursor }),
							page_size: input.page_size,
							scope: input.scope,
						})
						.pipe(Effect.mapError((cause) => cli_error(cause, input.selection.host)));

					if (!github_logins_match(page.viewer_login, input.selection.account_login)) {
						return yield* Effect.fail(
							provider_error("account_not_active", false, input.selection.host),
						);
					}

					if (page.repositories.length > input.page_size) {
						return yield* Effect.fail(
							provider_error("invalid_response", false, input.selection.host),
						);
					}

					const repositories = yield* Effect.forEach(page.repositories, (repository) =>
						map_repository(input.selection.host, repository),
					);
					const continuation =
						page.continuation.type === "complete"
							? ({ _tag: "complete" } as const)
							: ({
									_tag: "more",
									after: encode_cursor({
										account_login: input.selection.account_login,
										host: input.selection.host,
										native_cursor: page.continuation.cursor,
										page_size: input.page_size,
										scope: input.scope,
									}),
								} as const);

					return yield* ValidatePage(
						{ continuation, repositories },
						input.selection.host,
					);
				});
			const InspectSelectedRepository = (
				preparation: GitProviderClonePreparationResult,
				operation: GitProviderError["operation"],
				require_active_account = true,
			) =>
				Effect.gen(function* () {
					if (require_active_account) {
						yield* EnsureSelection(preparation.selection, operation);
					}

					const inspected = yield* cli
						.InspectRepository({
							account_login: preparation.selection.account_login,
							host: preparation.selection.host,
							name: preparation.repository.identity.name,
							owner: preparation.repository.identity.owner,
						})
						.pipe(
							Effect.mapError((cause) =>
								cli_error(cause, preparation.selection.host, operation),
							),
						);

					if (
						!github_logins_match(
							inspected.viewer_login,
							preparation.selection.account_login,
						)
					) {
						return yield* Effect.fail(
							provider_operation_error(
								operation,
								"account_not_active",
								false,
								preparation.selection.host,
							),
						);
					}

					const repository = yield* map_repository(
						preparation.selection.host,
						inspected.repository,
						operation,
					);

					if (!repositories_match(preparation.repository, repository)) {
						return yield* Effect.fail(
							provider_operation_error(
								operation,
								"stale_repository",
								false,
								preparation.selection.host,
							),
						);
					}

					return repository;
				});
			const PrepareClone = (unknown_input: GitProviderCloneRequestInput) =>
				Effect.gen(function* () {
					const input = yield* Schema.decodeUnknownEffect(GitProviderCloneRequest, {
						onExcessProperty: "error",
					})(unknown_input).pipe(
						Effect.mapError(() =>
							provider_operation_error("prepare_clone", "invalid_input", false),
						),
					);
					const preparation = {
						repository: input.repository,
						selection: input.selection,
					};

					if (
						input.selection.provider_id !== github_provider_id ||
						input.repository.identity.provider_id !== github_provider_id ||
						input.repository.origin.provider_id !== github_provider_id ||
						input.repository.identity.host !== input.selection.host
					) {
						return yield* Effect.fail(
							provider_operation_error(
								"prepare_clone",
								"invalid_input",
								false,
								input.selection.host,
							),
						);
					}

					const repository = yield* InspectSelectedRepository(
						preparation,
						"prepare_clone",
					);

					return yield* Schema.decodeUnknownEffect(GitProviderClonePreparation, {
						onExcessProperty: "error",
					})({ repository, selection: input.selection }).pipe(
						Effect.mapError(() =>
							provider_operation_error(
								"prepare_clone",
								"invalid_response",
								false,
								input.selection.host,
							),
						),
					);
				});
			const Clone = (unknown_input: GitProviderCloneExecutionInput) =>
				Effect.gen(function* () {
					const input = yield* Schema.decodeUnknownEffect(GitProviderCloneExecution, {
						onExcessProperty: "error",
					})(unknown_input).pipe(
						Effect.mapError(() =>
							provider_operation_error("clone_repository", "invalid_input", false),
						),
					);

					yield* InspectSelectedRepository(input.preparation, "clone_repository");

					return yield* Effect.uninterruptibleMask((restore) =>
						restore(
							Effect.scoped(
								Effect.gen(function* () {
									const execution = yield* cli
										.CloneRepository({
											account_login:
												input.preparation.selection.account_login,
											destination: input.destination,
											host: input.preparation.selection.host,
											name: input.preparation.repository.identity.name,
											owner: input.preparation.repository.identity.owner,
										})
										.pipe(
											Effect.mapError((cause) =>
												cli_error(
													cause,
													input.preparation.selection.host,
													"clone_repository",
												),
											),
										);
									const repository = yield* InspectSelectedRepository(
										input.preparation,
										"clone_repository",
										false,
									).pipe(
										Effect.mapError(() =>
											provider_operation_error(
												"clone_repository",
												"outcome_unknown",
												false,
												input.preparation.selection.host,
											),
										),
									);

									yield* execution.VerifyCheckout.pipe(
										Effect.mapError(() =>
											provider_operation_error(
												"clone_repository",
												"outcome_unknown",
												false,
												input.preparation.selection.host,
											),
										),
									);

									return yield* Schema.decodeUnknownEffect(
										GitProviderCloneResult,
										{
											onExcessProperty: "error",
										},
									)({
										canonical_root: execution.canonical_root,
										output_complete: execution.output_complete,
										repository,
										type: "cloned",
									}).pipe(
										Effect.mapError(() =>
											provider_operation_error(
												"clone_repository",
												"outcome_unknown",
												false,
												input.preparation.selection.host,
											),
										),
									);
								}),
							),
						).pipe(
							Effect.matchCauseEffect({
								onFailure: (cause) =>
									Cause.hasInterrupts(cause)
										? Effect.fail(
												provider_operation_error(
													"clone_repository",
													"outcome_unknown",
													false,
													input.preparation.selection.host,
												),
											)
										: Effect.failCause(cause),
								onSuccess: Effect.succeed,
							}),
						),
					);
				});
			const ReadPullRequest = (unknown_input: GitProviderPullRequestReadInput) =>
				Effect.gen(function* () {
					const input = yield* Schema.decodeUnknownEffect(GitProviderPullRequestRead, {
						onExcessProperty: "error",
					})(unknown_input).pipe(
						Effect.mapError(() =>
							provider_operation_error("read_pull_request", "invalid_input", false),
						),
					);

					if (
						input.selection.provider_id !== github_provider_id ||
						input.repository.provider_id !== github_provider_id ||
						input.repository.host !== input.selection.host
					) {
						return yield* Effect.fail(
							provider_operation_error(
								"read_pull_request",
								"invalid_input",
								false,
								input.selection.host,
							),
						);
					}

					yield* EnsureSelection(input.selection, "read_pull_request");
					const result = yield* cli
						.ReadPullRequest({
							host: input.selection.host,
							name: input.repository.name,
							owner: input.repository.owner,
							selected_branch: input.selected_branch,
						})
						.pipe(
							Effect.mapError((cause) =>
								cli_error(cause, input.selection.host, "read_pull_request"),
							),
						);

					if (!github_logins_match(result.viewer_login, input.selection.account_login)) {
						return yield* Effect.fail(
							provider_operation_error(
								"read_pull_request",
								"account_not_active",
								false,
								input.selection.host,
							),
						);
					}

					if (result.type === "no_pull_request") {
						return yield* ValidatePullRequest(
							{
								association: { _tag: "none" },
								branch: input.selected_branch,
								expected_head_commit: input.expected_head,
								repository: input.repository,
							},
							input.selection.host,
						);
					}

					if (result.type === "ambiguous_pull_requests") {
						return yield* ValidatePullRequest(
							{
								association: {
									_tag: "ambiguous",
									candidates: result.candidates.map((candidate) => ({
										base_branch: candidate.baseRefName,
										draft: candidate.isDraft,
										head_branch: candidate.headRefName,
										head_commit: candidate.headRefOid,
										number: candidate.number,
										origin: {
											native_id: candidate.id,
											provider_id: github_provider_id,
											resource_kind: "pull_request",
										},
										state: pull_request_state(
											candidate.state,
											candidate.isMerged,
										),
										title: candidate.title,
										web_url: candidate.url,
									})),
									candidates_truncated: !result.complete,
								},
								branch: input.selected_branch,
								expected_head_commit: input.expected_head,
								repository: input.repository,
							},
							input.selection.host,
						);
					}

					if (result.pull_request.headRefName !== input.selected_branch) {
						return yield* Effect.fail(
							provider_operation_error(
								"read_pull_request",
								"invalid_response",
								false,
								input.selection.host,
							),
						);
					}

					return yield* ValidatePullRequest(
						MapMatchedPullRequest(input, result.pull_request),
						input.selection.host,
					);
				});
			const ReadPullRequestTarget = (unknown_input: GitProviderPullRequestTargetReadInput) =>
				Effect.gen(function* () {
					const input = yield* Schema.decodeUnknownEffect(
						GitProviderPullRequestTargetRead,
						{
							onExcessProperty: "error",
						},
					)(unknown_input).pipe(
						Effect.mapError(() =>
							provider_operation_error(
								"read_pull_request_target",
								"invalid_input",
								false,
							),
						),
					);

					if (
						input.selection.provider_id !== github_provider_id ||
						input.repository.provider_id !== github_provider_id ||
						input.pull_request_origin.provider_id !== github_provider_id ||
						input.pull_request_origin.resource_kind !== "pull_request" ||
						input.repository.host !== input.selection.host
					) {
						return yield* Effect.fail(
							provider_operation_error(
								"read_pull_request_target",
								"invalid_input",
								false,
								input.selection.host,
							),
						);
					}

					yield* EnsureSelection(input.selection, "read_pull_request_target");
					const result = yield* cli
						.ReadPullRequestTarget({
							host: input.selection.host,
							name: input.repository.name,
							owner: input.repository.owner,
							pull_request_number: input.pull_request_number,
							pull_request_native_id: input.pull_request_origin.native_id,
							selected_branch: input.selected_branch,
						})
						.pipe(
							Effect.mapError((cause) =>
								cli_error(cause, input.selection.host, "read_pull_request_target"),
							),
						);

					if (!github_logins_match(result.viewer_login, input.selection.account_login)) {
						return yield* Effect.fail(
							provider_operation_error(
								"read_pull_request_target",
								"account_not_active",
								false,
								input.selection.host,
							),
						);
					}

					if (
						result.pull_request.number !== input.pull_request_number ||
						result.pull_request.id !== input.pull_request_origin.native_id ||
						result.pull_request.headRefName !== input.selected_branch ||
						result.pull_request.headRepository === null ||
						result.pull_request.headRepository.name.toLowerCase() !==
							input.repository.name.toLowerCase() ||
						result.pull_request.headRepository.owner.login.toLowerCase() !==
							input.repository.owner.toLowerCase()
					) {
						return yield* Effect.fail(
							provider_operation_error(
								"read_pull_request_target",
								"invalid_response",
								false,
								input.selection.host,
							),
						);
					}

					return yield* ValidatePullRequest(
						MapMatchedPullRequest(input, result.pull_request),
						input.selection.host,
						"read_pull_request_target",
					);
				});

			return {
				Descriptor: {
					capabilities: [
						{ _tag: "available", capability: "inspect_authentication" },
						{ _tag: "available", capability: "discover_repositories" },
						{ _tag: "available", capability: "clone_repository" },
						{ _tag: "available", capability: "read_reviews" },
						{ _tag: "available", capability: "read_ci" },
						{
							_tag: "unavailable",
							capability: "write_provider_mutations",
							reason: "Provider writes require a separate approval boundary",
						},
					],
					display_name: "GitHub",
					provider_id: github_provider_id,
				},
				Clone,
				DiscoverRepositories,
				Inspect,
				PrepareClone,
				ReadPullRequest,
				ReadPullRequestTarget,
			};
		}),
	);
}
