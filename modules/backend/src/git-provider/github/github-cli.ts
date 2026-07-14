import { Buffer } from "node:buffer";

import {
	Cause,
	Context,
	Crypto,
	Data,
	Effect,
	FileSystem,
	Layer,
	Option,
	Path,
	Schema,
	Scope,
} from "effect";

import {
	ReadFileIdentity,
	same_file_identity,
	type FileIdentity,
} from "../../filesystem/file-identity";
import { ProcessRunner, type ProcessRunnerResult } from "../../git/process-runner";
import {
	GitProviderCloneDestinationProof,
	type GitProviderCloneDestinationProof as GitProviderCloneDestinationProofValue,
	type GitProviderDiscoveryScope,
} from "../git-provider";
import { GitHubCliExecutable, GitHubCliGitExecutable } from "./github-cli-executable";

const VersionPattern = /^gh version (\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)(?:\s|$)/mu;
const clone_receipt_name = "artisan-clone-receipt";
const null_device_path = process.platform === "win32" ? "NUL" : "/dev/null";
/** Streams the selected account credential between child processes without exposing it to Artisan. */
const account_credential_helper = [
	"!f() {",
	"protocol=;",
	"host=;",
	"while IFS='=' read -r key value; do",
	'test -n "$key" || break;',
	'case "$key" in',
	'protocol) protocol="$value" ;;',
	'host) host="$value" ;;',
	"esac;",
	"done;",
	'if test "$1" = get && test "$protocol" = https && test "$host" = "$ARTISAN_GH_HOST"; then',
	"printf 'username=x-access-token\\npassword=';",
	'GH_CONFIG_DIR="$ARTISAN_GH_CONFIG_DIR" "$ARTISAN_GH_EXECUTABLE" auth token --hostname "$ARTISAN_GH_HOST" --user "$ARTISAN_GH_ACCOUNT";',
	"fi;",
	"};",
	"f",
].join(" ");
const inherited_environment_keys = [
	"APPDATA",
	"COMSPEC",
	"GH_CONFIG_DIR",
	"HOME",
	"HOMEDRIVE",
	"HOMEPATH",
	"LANG",
	"LC_ALL",
	"LC_CTYPE",
	"LOCALAPPDATA",
	"PATH",
	"PATHEXT",
	"PROGRAMDATA",
	"PROGRAMFILES",
	"PROGRAMFILES(X86)",
	"SSL_CERT_DIR",
	"SSL_CERT_FILE",
	"SYSTEMDRIVE",
	"SYSTEMROOT",
	"TEMP",
	"TMP",
	"TMPDIR",
	"USERPROFILE",
	"WINDIR",
	"XDG_CONFIG_HOME",
] as const;

const GitHubAuthEntry = Schema.Struct({
	active: Schema.Boolean,
	error: Schema.optional(Schema.String),
	gitProtocol: Schema.String,
	host: Schema.String,
	login: Schema.String,
	scopes: Schema.optional(Schema.String),
	state: Schema.Literals(["success", "timeout", "error"]),
	tokenSource: Schema.String,
});

const GitHubAuthStatus = Schema.Struct({
	hosts: Schema.Record(Schema.String, Schema.Array(GitHubAuthEntry)),
});

const GitHubGraphqlError = Schema.Struct({
	extensions: Schema.optional(
		Schema.Struct({
			type: Schema.optional(Schema.String),
		}),
	),
	message: Schema.String,
	type: Schema.optional(Schema.String),
});

const GitHubGraphqlEnvelope = Schema.Struct({
	data: Schema.optional(Schema.NullOr(Schema.Unknown)),
	errors: Schema.optional(Schema.Array(GitHubGraphqlError)),
});

const GitHubPageInfo = Schema.Struct({
	endCursor: Schema.NullOr(Schema.String),
	hasNextPage: Schema.Boolean,
});

const GitHubRepositoryNode = Schema.Struct({
	defaultBranchRef: Schema.NullOr(
		Schema.Struct({
			name: Schema.NonEmptyString,
		}),
	),
	id: Schema.NonEmptyString,
	isArchived: Schema.Boolean,
	name: Schema.NonEmptyString,
	nameWithOwner: Schema.NonEmptyString,
	owner: Schema.Struct({
		login: Schema.NonEmptyString,
	}),
	sshUrl: Schema.NonEmptyString,
	updatedAt: Schema.NonEmptyString,
	url: Schema.NonEmptyString,
	viewerPermission: Schema.NullOr(
		Schema.Literals(["ADMIN", "MAINTAIN", "WRITE", "TRIAGE", "READ"]),
	),
	visibility: Schema.Literals(["PUBLIC", "PRIVATE", "INTERNAL"]),
});

const GitHubRepositoryConnection = Schema.Struct({
	nodes: Schema.Array(Schema.NullOr(GitHubRepositoryNode)),
	pageInfo: GitHubPageInfo,
});

const GitHubAccountRepositoryData = Schema.Struct({
	viewer: Schema.Struct({
		login: Schema.NonEmptyString,
		repositories: GitHubRepositoryConnection,
	}),
});

const GitHubOrganizationRepositoryData = Schema.Struct({
	organization: Schema.NullOr(
		Schema.Struct({
			repositories: GitHubRepositoryConnection,
		}),
	),
	viewer: Schema.Struct({
		login: Schema.NonEmptyString,
	}),
});

const GitHubSearchRepositoryData = Schema.Struct({
	search: GitHubRepositoryConnection,
	viewer: Schema.Struct({
		login: Schema.NonEmptyString,
	}),
});

const GitHubSingleRepositoryData = Schema.Struct({
	repository: Schema.NullOr(GitHubRepositoryNode),
	viewer: Schema.Struct({
		login: Schema.NonEmptyString,
	}),
});

const GitHubBoundedText = (maximum_bytes: number) =>
	Schema.String.check(
		Schema.makeFilter<string>((value) =>
			value.length === 0 ||
			Buffer.byteLength(value, "utf8") > maximum_bytes ||
			value.includes("\0")
				? `Expected non-empty text bounded to ${maximum_bytes} bytes`
				: undefined,
		),
	);
const GitHubNativeId = GitHubBoundedText(512);
const GitHubName = GitHubBoundedText(512);
const GitHubTitle = GitHubBoundedText(1_024);
const GitHubPath = GitHubBoundedText(4_096);
const GitHubMessage = GitHubBoundedText(4_096);
const GitHubUrl = GitHubBoundedText(2_048);
const GitHubDateTime = GitHubBoundedText(128);
const GitHubNonNegativeInt = Schema.Int.check(Schema.isGreaterThanOrEqualTo(0));
const GitHubPositiveInt = Schema.Int.check(Schema.isGreaterThanOrEqualTo(1));

const GitHubBoundedArray = <S extends Schema.Top>(schema: S, maximum: number) =>
	Schema.Array(schema).check(Schema.isMaxLength(maximum));

const GitHubPullRequestCandidateData = Schema.Struct({
	baseRefName: GitHubName,
	headRefOid: GitHubNativeId,
	headRefName: GitHubName,
	headRepository: Schema.NullOr(
		Schema.Struct({ name: GitHubName, owner: Schema.Struct({ login: GitHubName }) }),
	),
	id: GitHubNativeId,
	isDraft: Schema.Boolean,
	isMerged: Schema.Boolean,
	number: GitHubPositiveInt,
	state: Schema.Literals(["OPEN", "CLOSED", "MERGED"]),
	title: GitHubTitle,
	url: GitHubUrl,
});

const GitHubPullRequestAssociationData = Schema.Struct({
	repository: Schema.NullOr(
		Schema.Struct({
			pullRequests: Schema.Struct({
				nodes: GitHubBoundedArray(GitHubPullRequestCandidateData, 10),
				pageInfo: GitHubPageInfo,
			}),
		}),
	),
	viewer: Schema.Struct({ login: Schema.NonEmptyString }),
});

const GitHubRequestedReviewer = Schema.Union([
	Schema.Struct({ __typename: Schema.Literal("User"), login: GitHubName }),
	Schema.Struct({
		__typename: Schema.Literal("Team"),
		organization: Schema.Struct({ login: GitHubName }),
		slug: GitHubName,
	}),
	Schema.Struct({ __typename: Schema.Literals(["Bot", "EnterpriseTeam", "Mannequin"]) }),
]);

const GitHubReviewRequest = Schema.Struct({
	requestedReviewer: Schema.NullOr(GitHubRequestedReviewer),
});

const GitHubReview = Schema.Struct({
	author: Schema.NullOr(Schema.Struct({ login: GitHubName })),
	commit: Schema.NullOr(Schema.Struct({ oid: GitHubNativeId })),
	id: GitHubNativeId,
	state: Schema.Literals(["APPROVED", "CHANGES_REQUESTED", "COMMENTED", "DISMISSED"]),
	submittedAt: GitHubDateTime,
});

const GitHubReviewThread = Schema.Struct({
	comments: Schema.Struct({
		nodes: GitHubBoundedArray(
			Schema.Struct({ id: GitHubNativeId, updatedAt: GitHubDateTime }),
			1,
		),
		totalCount: GitHubNonNegativeInt,
	}),
	id: GitHubNativeId,
	isOutdated: Schema.Boolean,
	isResolved: Schema.Boolean,
	line: Schema.NullOr(GitHubPositiveInt),
	path: GitHubPath,
	subjectType: Schema.Literals(["FILE", "LINE"]),
});

const GitHubCheckAnnotation = Schema.Struct({
	annotationLevel: Schema.NullOr(Schema.Literals(["NOTICE", "WARNING", "FAILURE"])),
	location: Schema.Struct({
		end: Schema.Struct({ line: GitHubPositiveInt }),
		start: Schema.Struct({ line: GitHubPositiveInt }),
	}),
	message: GitHubMessage,
	path: GitHubPath,
	title: Schema.NullOr(GitHubName),
});

const GitHubCheckRun = Schema.Struct({
	__typename: Schema.Literal("CheckRun"),
	annotations: Schema.NullOr(
		Schema.Struct({
			nodes: GitHubBoundedArray(GitHubCheckAnnotation, 50),
			pageInfo: GitHubPageInfo,
			totalCount: GitHubNonNegativeInt,
		}),
	),
	checkSuite: Schema.Struct({
		app: Schema.NullOr(Schema.Struct({ name: GitHubName })),
		id: GitHubNativeId,
		workflowRun: Schema.NullOr(
			Schema.Struct({
				id: GitHubNativeId,
				runAttempt: GitHubPositiveInt,
				url: GitHubUrl,
				workflow: Schema.Struct({ name: GitHubName }),
			}),
		),
	}),
	completedAt: Schema.NullOr(GitHubDateTime),
	conclusion: Schema.NullOr(GitHubName),
	detailsUrl: Schema.NullOr(GitHubUrl),
	id: GitHubNativeId,
	isRequired: Schema.Boolean,
	name: GitHubName,
	startedAt: Schema.NullOr(GitHubDateTime),
	status: GitHubName,
});

const GitHubStatusContext = Schema.Struct({
	__typename: Schema.Literal("StatusContext"),
	context: GitHubName,
	id: GitHubNativeId,
	isRequired: Schema.Boolean,
	state: GitHubName,
	targetUrl: Schema.NullOr(GitHubUrl),
});

const GitHubPullRequestDetail = Schema.Struct({
	baseRefName: GitHubName,
	baseRefOid: GitHubNativeId,
	commits: Schema.Struct({
		nodes: GitHubBoundedArray(
			Schema.Struct({
				commit: Schema.Struct({
					statusCheckRollup: Schema.NullOr(
						Schema.Struct({
							contexts: Schema.Struct({
								nodes: GitHubBoundedArray(
									Schema.Union([GitHubCheckRun, GitHubStatusContext]),
									100,
								),
								pageInfo: GitHubPageInfo,
								totalCount: GitHubNonNegativeInt,
							}),
						}),
					),
				}),
			}),
			1,
		),
	}).check(
		Schema.makeFilter((value) =>
			value.nodes.length === 1 ? undefined : "Expected one pull request commit",
		),
	),
	headRefName: GitHubName,
	headRefOid: GitHubNativeId,
	headRepository: Schema.NullOr(
		Schema.Struct({ name: GitHubName, owner: Schema.Struct({ login: GitHubName }) }),
	),
	id: GitHubNativeId,
	isDraft: Schema.Boolean,
	isMerged: Schema.Boolean,
	mergeable: Schema.Literals(["MERGEABLE", "CONFLICTING", "UNKNOWN"]),
	number: GitHubPositiveInt,
	requestedReviewers: Schema.Struct({
		nodes: GitHubBoundedArray(GitHubReviewRequest, 100),
		pageInfo: GitHubPageInfo,
		totalCount: GitHubNonNegativeInt,
	}),
	reviewDecision: Schema.NullOr(
		Schema.Literals(["APPROVED", "CHANGES_REQUESTED", "REVIEW_REQUIRED"]),
	),
	reviewThreads: Schema.Struct({
		nodes: GitHubBoundedArray(GitHubReviewThread, 100),
		pageInfo: GitHubPageInfo,
		totalCount: GitHubNonNegativeInt,
	}),
	reviews: Schema.Struct({
		nodes: GitHubBoundedArray(GitHubReview, 100),
		pageInfo: GitHubPageInfo,
		totalCount: GitHubNonNegativeInt,
	}),
	state: Schema.Literals(["OPEN", "CLOSED", "MERGED"]),
	title: GitHubTitle,
	url: GitHubUrl,
});

const GitHubPullRequestDetailData = Schema.Struct({
	repository: Schema.NullOr(
		Schema.Struct({
			pullRequest: Schema.NullOr(GitHubPullRequestDetail),
		}),
	),
	viewer: Schema.Struct({ login: Schema.NonEmptyString }),
});

const GitHubCheckFailureDetailData = Schema.Struct({
	repository: Schema.NullOr(
		Schema.Struct({
			id: GitHubNativeId,
			nameWithOwner: GitHubName,
			pullRequest: Schema.NullOr(
				Schema.Struct({
					headRefName: GitHubName,
					headRefOid: GitHubNativeId,
					headRepository: Schema.NullOr(
						Schema.Struct({
							name: GitHubName,
							owner: Schema.Struct({ login: GitHubName }),
						}),
					),
					id: GitHubNativeId,
					number: GitHubPositiveInt,
				}),
			),
		}),
	),
	viewer: Schema.Struct({ login: Schema.NonEmptyString }),
	checkRun: Schema.NullOr(
		Schema.Struct({
			__typename: Schema.Literal("CheckRun"),
			completedAt: Schema.NullOr(GitHubDateTime),
			databaseId: Schema.NullOr(GitHubPositiveInt),
			id: GitHubNativeId,
			name: GitHubName,
			summary: Schema.NullOr(GitHubBoundedText(256 * 1024)),
			text: Schema.NullOr(GitHubBoundedText(256 * 1024)),
			title: Schema.NullOr(GitHubBoundedText(16 * 1024)),
			checkSuite: Schema.Struct({
				commit: Schema.Struct({
					oid: GitHubNativeId,
					repository: Schema.NullOr(
						Schema.Struct({ id: GitHubNativeId, nameWithOwner: GitHubName }),
					),
				}),
				id: GitHubNativeId,
				workflowRun: Schema.NullOr(
					Schema.Struct({
						databaseId: Schema.NullOr(GitHubPositiveInt),
						id: GitHubNativeId,
						runAttempt: GitHubPositiveInt,
					}),
				),
			}),
			status: GitHubName,
		}),
	),
});

const repository_fields = `
id
name
nameWithOwner
isArchived
visibility
url
sshUrl
updatedAt
viewerPermission
defaultBranchRef { name }
owner { login }
`;

const account_query = `
query ArtisanRepositoryDiscovery($first: Int!, $after: String) {
  viewer {
    login
    repositories(
      first: $first
      after: $after
      affiliations: [OWNER, COLLABORATOR, ORGANIZATION_MEMBER]
      orderBy: { field: UPDATED_AT, direction: DESC }
    ) {
      nodes { ${repository_fields} }
      pageInfo { hasNextPage endCursor }
    }
  }
}
`;

const organization_query = `
query ArtisanOrganizationRepositoryDiscovery(
  $organization: String!
  $first: Int!
  $after: String
) {
  viewer { login }
  organization(login: $organization) {
    repositories(
      first: $first
      after: $after
      orderBy: { field: UPDATED_AT, direction: DESC }
    ) {
      nodes { ${repository_fields} }
      pageInfo { hasNextPage endCursor }
    }
  }
}
`;

const search_query = `
query ArtisanRepositorySearch($search: String!, $first: Int!, $after: String) {
  viewer { login }
  search(type: REPOSITORY, query: $search, first: $first, after: $after) {
    nodes { ... on Repository { ${repository_fields} } }
    pageInfo { hasNextPage endCursor }
  }
}
`;

const repository_query = `
query ArtisanRepository($owner: String!, $name: String!) {
  viewer { login }
  repository(owner: $owner, name: $name) { ${repository_fields} }
}
`;

const pull_request_association_query = `
query ArtisanPullRequestAssociation($owner: String!, $name: String!, $branch: String!) {
  viewer { login }
  repository(owner: $owner, name: $name) {
    pullRequests(headRefName: $branch, first: 10, orderBy: { field: UPDATED_AT, direction: DESC }) {
      nodes { id number title url state isDraft isMerged: merged baseRefName headRefName headRefOid headRepository { name owner { login } } }
      pageInfo { hasNextPage endCursor }
    }
  }
}
`;

const pull_request_detail_query = `
query ArtisanPullRequestDetail($owner: String!, $name: String!, $number: Int!) {
  viewer { login }
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      id number title url state isDraft isMerged: merged mergeable reviewDecision
      baseRefName baseRefOid headRefName headRefOid headRepository { name owner { login } }
      requestedReviewers: reviewRequests(first: 100) {
        totalCount
        nodes {
          requestedReviewer {
            __typename
            ... on User { login }
            ... on Team { slug organization { login } }
          }
        }
        pageInfo { hasNextPage endCursor }
      }
      reviews(first: 100, states: [APPROVED, CHANGES_REQUESTED, COMMENTED, DISMISSED]) {
        totalCount
        nodes { id author { login } commit { oid } state submittedAt }
        pageInfo { hasNextPage endCursor }
      }
      reviewThreads(first: 100) {
        totalCount
        nodes {
          id isResolved isOutdated path line subjectType
          comments(last: 1) { totalCount nodes { id updatedAt } }
        }
        pageInfo { hasNextPage endCursor }
      }
      commits(last: 1) {
        nodes {
          commit {
            statusCheckRollup {
              contexts(first: 100) {
                totalCount
                nodes {
                  __typename
                  ... on CheckRun {
                    id name status conclusion detailsUrl
                    isRequired(pullRequestNumber: $number)
                    startedAt completedAt
                    checkSuite {
                      id
                      app { name }
                      workflowRun { id url runAttempt workflow { name } }
                    }
                    annotations(first: 50) {
                      totalCount
                      nodes {
                        annotationLevel
                        location { start { line } end { line } }
                        path title message
                      }
                      pageInfo { hasNextPage endCursor }
                    }
                  }
                  ... on StatusContext {
                    id context state targetUrl
                    isRequired(pullRequestNumber: $number)
                  }
                }
                pageInfo { hasNextPage endCursor }
              }
            }
          }
        }
      }
    }
  }
}
`;

const check_failure_detail_query = `
query ArtisanCheckFailureDetail($owner: String!, $name: String!, $number: Int!, $check: ID!) {
  viewer { login }
  repository(owner: $owner, name: $name) {
    id nameWithOwner
    pullRequest(number: $number) { id number headRefName headRefOid headRepository { name owner { login } } }
  }
  checkRun: node(id: $check) {
    __typename
    ... on CheckRun {
      id databaseId name status completedAt
      title summary text
      checkSuite {
        id
        commit { oid repository { id nameWithOwner } }
        workflowRun { id databaseId runAttempt }
      }
    }
  }
}
`;

/** Reports one account discovered from GitHub CLI without exposing its token source or token. */
export type GitHubCliAccount =
	| {
			readonly active: boolean;
			readonly git_protocol: "https" | "ssh" | "unknown";
			readonly host: string;
			readonly login: string;
			readonly scopes: ReadonlyArray<string>;
			readonly type: "authenticated";
	  }
	| {
			readonly active: boolean;
			readonly host: string;
			readonly login?: string;
			readonly reason: "error" | "timeout";
			readonly type: "failed";
	  };

/** Groups GitHub CLI accounts by their exact configured host. */
export interface GitHubCliHostAuthentication {
	readonly accounts: ReadonlyArray<GitHubCliAccount>;
	readonly host: string;
}

/** Describes the local GitHub CLI dependency and its safe authentication projection. */
export type GitHubCliInspection =
	| {
			readonly command: string;
			readonly type: "missing";
	  }
	| {
			readonly executable_path: string;
			readonly hosts: ReadonlyArray<GitHubCliHostAuthentication>;
			readonly type: "available";
			readonly version: string;
	  }
	| {
			readonly executable_path: string;
			readonly reason: "required_features_missing";
			readonly type: "incompatible";
			readonly version: string;
	  }
	| {
			readonly command: string;
			readonly executable_path?: string;
			readonly reason:
				| "auth_probe_failed"
				| "invalid_output"
				| "process_failed"
				| "timed_out";
			readonly type: "unavailable";
			readonly version?: string;
	  };

/** Contains the provider fields required to produce a canonical repository projection. */
export interface GitHubCliRepository {
	readonly archived: boolean;
	readonly default_branch?: string;
	readonly name: string;
	readonly name_with_owner: string;
	readonly native_id: string;
	readonly owner: string;
	readonly ssh_url: string;
	readonly updated_at: string;
	readonly viewer_permission: "admin" | "maintain" | "write" | "triage" | "read" | "unknown";
	readonly visibility: "public" | "private" | "internal";
	readonly web_url: string;
}

/** Represents whether another provider-native repository page exists. */
export type GitHubCliRepositoryContinuation =
	| { readonly type: "complete" }
	| { readonly cursor: string; readonly type: "more" };

/** Returns one decoded GitHub repository page plus the account that actually served it. */
export interface GitHubCliRepositoryPage {
	readonly continuation: GitHubCliRepositoryContinuation;
	readonly repositories: ReadonlyArray<GitHubCliRepository>;
	readonly viewer_login: string;
}

/** Supplies an exact GitHub host, canonical scope, and provider-native continuation. */
export interface GitHubCliRepositoryQuery {
	readonly host: string;
	readonly native_cursor?: string;
	readonly page_size: number;
	readonly scope: GitProviderDiscoveryScope;
}

/** Identifies one exact repository for a fresh provider-side identity check. */
export interface GitHubCliRepositoryInspection {
	readonly account_login: string;
	readonly host: string;
	readonly name: string;
	readonly owner: string;
}

/** Returns one repository together with the account that actually observed it. */
export interface GitHubCliRepositoryInspectionResult {
	readonly repository: GitHubCliRepository;
	readonly viewer_login: string;
}

/** Carries a bounded branch association or detailed GitHub pull request projection. */
export type GitHubCliPullRequestReadResult =
	| { readonly type: "no_pull_request"; readonly viewer_login: string }
	| {
			readonly candidates: ReadonlyArray<typeof GitHubPullRequestCandidateData.Type>;
			readonly complete: boolean;
			readonly type: "ambiguous_pull_requests";
			readonly viewer_login: string;
	  }
	| {
			readonly pull_request: Exclude<
				Exclude<
					(typeof GitHubPullRequestDetailData.Type)["repository"],
					null
				>["pullRequest"],
				null
			>;
			readonly type: "matched_pull_request";
			readonly viewer_login: string;
	  };

/** Selects one repository and branch for a two-step, exact-number GitHub PR read. */
export interface GitHubCliPullRequestRead {
	readonly host: string;
	readonly name: string;
	readonly owner: string;
	readonly selected_branch: string;
}

/** Selects one exact GitHub pull request for a direct detailed read. */
export interface GitHubCliPullRequestTargetRead extends GitHubCliPullRequestRead {
	readonly pull_request_number: number;
	readonly pull_request_native_id: string;
}

/** Binds one failed-check read to an exact pull request, branch, head, and check node. */
export interface GitHubCliCheckFailureDetailRead extends GitHubCliPullRequestTargetRead {
	readonly check_native_id: string;
	readonly expected_head: string;
}

/** Carries bounded, sanitized check output without preserving the provider payload. */
export interface GitHubCliCheckFailureDetail {
	readonly attempt?: number;
	readonly check_native_id: string;
	readonly head_commit: string;
	readonly log:
		| {
				readonly _tag: "available";
				readonly observed_bytes: number;
				readonly truncated: boolean;
				readonly untrusted_excerpt: string;
		  }
		| {
				readonly _tag: "unavailable";
				readonly reason: "check_not_completed" | "not_actions_job" | "not_available";
		  };
	readonly name: string;
	readonly output: {
		readonly summary:
			| {
					readonly _tag: "available";
					readonly truncated: boolean;
					readonly untrusted_text: string;
			  }
			| { readonly _tag: "unavailable" };
		readonly text:
			| {
					readonly _tag: "available";
					readonly truncated: boolean;
					readonly untrusted_text: string;
			  }
			| { readonly _tag: "unavailable" };
		readonly title?: string;
	};
	readonly viewer_login: string;
	readonly workflow_native_id?: string;
}

/** Supplies the approved paths and exact repository for one GitHub CLI clone. */
export interface GitHubCliCloneInput {
	readonly account_login: string;
	readonly destination: GitProviderCloneDestinationProofValue;
	readonly host: string;
	readonly name: string;
	readonly owner: string;
}

/** Retains only bounded execution completeness after a successful clone. */
export interface GitHubCliCloneResult {
	readonly VerifyCheckout: Effect.Effect<void, GitHubCliError, Scope.Scope>;
	readonly canonical_root: string;
	readonly output_complete: boolean;
}

/** Classifies GitHub CLI failures without retaining provider output in the error. */
export class GitHubCliError extends Data.TaggedError("GitHubCliError")<{
	readonly operation:
		| "clone_repository"
		| "inspect_repository"
		| "query_repositories"
		| "read_check_failure_detail"
		| "read_pull_request"
		| "read_pull_request_target";
	readonly reason:
		| "authentication_required"
		| "dependency_incompatible"
		| "dependency_missing"
		| "git_dependency_missing"
		| "invalid_response"
		| "invalid_destination"
		| "network_unavailable"
		| "outcome_unknown"
		| "permission_insufficient"
		| "process_failed"
		| "rate_limited"
		| "remote_not_found"
		| "remote_rejected"
		| "timed_out";
	readonly retryable: boolean;
}> {}

/** Rejects invalid process limits before a GitHub CLI service is published. */
export class GitHubCliConfigurationError extends Data.TaggedError("GitHubCliConfigurationError")<{
	readonly field: "clone_timeout_ms" | "probe_timeout_ms" | "request_timeout_ms";
}> {}

/** Owns GitHub CLI reads, authenticated Git clone execution, and provider-native decoding. */
export class GitHubCli extends Context.Service<
	GitHubCli,
	{
		readonly Inspect: Effect.Effect<GitHubCliInspection>;
		readonly CloneRepository: (
			input: GitHubCliCloneInput,
		) => Effect.Effect<GitHubCliCloneResult, GitHubCliError, Scope.Scope>;
		readonly InspectRepository: (
			input: GitHubCliRepositoryInspection,
		) => Effect.Effect<GitHubCliRepositoryInspectionResult, GitHubCliError>;
		readonly QueryRepositories: (
			input: GitHubCliRepositoryQuery,
		) => Effect.Effect<GitHubCliRepositoryPage, GitHubCliError>;
		readonly ReadPullRequest: (
			input: GitHubCliPullRequestRead,
		) => Effect.Effect<GitHubCliPullRequestReadResult, GitHubCliError>;
		readonly ReadPullRequestTarget: (
			input: GitHubCliPullRequestTargetRead,
		) => Effect.Effect<
			Extract<GitHubCliPullRequestReadResult, { readonly type: "matched_pull_request" }>,
			GitHubCliError
		>;
		readonly ReadCheckFailureDetail: (
			input: GitHubCliCheckFailureDetailRead,
		) => Effect.Effect<GitHubCliCheckFailureDetail, GitHubCliError>;
	}
>()("Artisan/GitHubCli") {}

/** Configures bounded, non-interactive GitHub CLI subprocesses. */
export interface GitHubCliOptions {
	readonly clone_timeout_ms?: number;
	readonly command?: string;
	readonly cwd: string;
	readonly probe_timeout_ms?: number;
	readonly projects_root?: string;
	readonly request_timeout_ms?: number;
}

function selected_environment() {
	const inherited: Record<string, string> = {};

	for (const key of inherited_environment_keys) {
		const value = process.env[key];

		if (value !== undefined) {
			inherited[key] = value;
		}
	}

	return {
		...inherited,
		GCM_INTERACTIVE: "Never",
		GH_NO_UPDATE_NOTIFIER: "1",
		GH_PAGER: "cat",
		GH_PROMPT_DISABLED: "1",
		GIT_ATTR_NOSYSTEM: "1",
		GIT_CONFIG_COUNT: "0",
		GIT_CONFIG_GLOBAL: null_device_path,
		GIT_CONFIG_NOSYSTEM: "1",
		GIT_CONFIG_SYSTEM: null_device_path,
		GIT_TERMINAL_PROMPT: "0",
		NO_COLOR: "1",
		NoDefaultCurrentDirectoryInExePath: "1",
		PAGER: "cat",
	};
}

function github_cli_config_directory(
	environment: Readonly<Record<string, string>>,
	path_service: Path.Path,
	cwd: string,
) {
	const configured = environment.GH_CONFIG_DIR;
	const xdg = environment.XDG_CONFIG_HOME;
	const app_data = environment.APPDATA;
	const home = environment.HOME;
	const candidate =
		configured !== undefined && configured.length > 0
			? configured
			: xdg !== undefined && xdg.length > 0
				? path_service.join(xdg, "gh")
				: process.platform === "win32" && app_data !== undefined && app_data.length > 0
					? path_service.join(app_data, "GitHub CLI")
					: home !== undefined && home.length > 0
						? path_service.join(home, ".config", "gh")
						: undefined;

	return candidate === undefined
		? undefined
		: path_service.isAbsolute(candidate)
			? path_service.normalize(candidate)
			: path_service.resolve(cwd, candidate);
}

function git_shell_path(path: string) {
	return process.platform === "win32" ? path.replaceAll("\\", "/") : path;
}

function clone_environment(
	input: GitHubCliCloneInput,
	gh_path: string,
	git_path: string,
	path_service: Path.Path,
	private_home: string,
	gh_config_directory: string,
) {
	return {
		ARTISAN_GH_ACCOUNT: input.account_login,
		ARTISAN_GH_CONFIG_DIR: gh_config_directory,
		ARTISAN_GH_EXECUTABLE: git_shell_path(gh_path),
		ARTISAN_GH_HOST: input.host,
		APPDATA: path_service.join(private_home, "appdata"),
		GH_CONFIG_DIR: undefined,
		GIT_CONFIG_COUNT: "4",
		GIT_CONFIG_KEY_0: "core.hooksPath",
		GIT_CONFIG_VALUE_0: null_device_path,
		GIT_CONFIG_KEY_1: "credential.helper",
		GIT_CONFIG_VALUE_1: "",
		GIT_CONFIG_KEY_2: "credential.helper",
		GIT_CONFIG_VALUE_2: account_credential_helper,
		GIT_CONFIG_KEY_3: "http.followRedirects",
		GIT_CONFIG_VALUE_3: "false",
		HOME: private_home,
		HOMEDRIVE: undefined,
		HOMEPATH: undefined,
		NETRC: path_service.join(private_home, ".netrc"),
		PATH: path_service.dirname(git_path),
		USERPROFILE: private_home,
		XDG_CONFIG_HOME: path_service.join(private_home, "xdg"),
	};
}

function decode_text(value: Uint8Array) {
	return Buffer.from(value).toString("utf8");
}

function parse_json(value: Uint8Array) {
	return Effect.try(() => JSON.parse(decode_text(value)) as unknown);
}

function ParseSchema<S extends Schema.Top>(schema: S, value: unknown) {
	return Schema.decodeUnknownEffect(schema, { onExcessProperty: "ignore" })(value);
}

function ParseStrictSchema<S extends Schema.Top>(schema: S, value: unknown) {
	return Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })(value);
}

function ParseAuthStatus(stdout: Uint8Array) {
	return parse_json(stdout).pipe(
		Effect.flatMap((value) => ParseSchema(GitHubAuthStatus, value)),
		Effect.option,
	);
}

function ParseEnvelope(output: Uint8Array) {
	return parse_json(output).pipe(
		Effect.flatMap((value) => ParseSchema(GitHubGraphqlEnvelope, value)),
		Effect.option,
	);
}

function parse_version(stdout: Uint8Array) {
	return VersionPattern.exec(decode_text(stdout))?.[1];
}

function normalize_protocol(value: string): "https" | "ssh" | "unknown" {
	return value === "https" || value === "ssh" ? value : "unknown";
}

function normalize_scopes(value: string | undefined) {
	return [
		...new Set(
			(value ?? "")
				.split(",")
				.map((scope) => scope.trim())
				.filter((scope) => scope.length > 0),
		),
	].toSorted();
}

function normalize_accounts(
	host: string,
	accounts: ReadonlyArray<typeof GitHubAuthEntry.Type>,
): ReadonlyArray<GitHubCliAccount> {
	return accounts.map((account) =>
		account.state === "success"
			? {
					active: account.active,
					git_protocol: normalize_protocol(account.gitProtocol),
					host,
					login: account.login,
					scopes: normalize_scopes(account.scopes),
					type: "authenticated" as const,
				}
			: {
					active: account.active,
					host,
					...(account.login.length === 0 ? {} : { login: account.login }),
					reason: account.state,
					type: "failed" as const,
				},
	);
}

function incompatible_auth_status(result: ProcessRunnerResult) {
	const output = `${decode_text(result.stdout)}\n${decode_text(result.stderr)}`.toLowerCase();

	return output.includes("unknown flag") || output.includes("unknown command");
}

function api_error(
	reason: GitHubCliError["reason"],
	retryable: boolean,
	operation: GitHubCliError["operation"] = "query_repositories",
) {
	return new GitHubCliError({
		operation,
		reason,
		retryable,
	});
}

function classify_api_failure(
	result: ProcessRunnerResult,
	envelope: Option.Option<typeof GitHubGraphqlEnvelope.Type>,
	operation: GitHubCliError["operation"] = "query_repositories",
) {
	const messages = Option.match(envelope, {
		onNone: () => [] as ReadonlyArray<string>,
		onSome: (value) =>
			(value.errors ?? []).flatMap((error) => [
				error.message,
				error.type ?? "",
				error.extensions?.type ?? "",
			]),
	});
	const output = [decode_text(result.stdout), decode_text(result.stderr), ...messages]
		.join("\n")
		.toLowerCase();

	if (output.includes("rate limit") || output.includes("rate_limited")) {
		return api_error("rate_limited", true, operation);
	}

	if (
		output.includes("resource not accessible") ||
		output.includes("forbidden") ||
		output.includes("http 403")
	) {
		return api_error("permission_insufficient", false, operation);
	}

	if (
		output.includes("authentication") ||
		output.includes("not logged") ||
		output.includes("bad credentials") ||
		output.includes("http 401")
	) {
		return api_error("authentication_required", false, operation);
	}

	if (
		output.includes("could not resolve") ||
		output.includes("connection refused") ||
		output.includes("failed to connect") ||
		output.includes("network is unreachable")
	) {
		return api_error("network_unavailable", true, operation);
	}

	if (output.includes("not found") || output.includes("not_found")) {
		return api_error("remote_not_found", false, operation);
	}

	const remote_temporarily_unavailable =
		/\bhttp(?: status)?[\s:]+5\d{2}\b/u.test(output) ||
		output.includes("internal server error") ||
		output.includes("temporarily unavailable") ||
		output.includes("service unavailable") ||
		output.includes("bad gateway") ||
		output.includes("gateway timeout");

	return api_error("remote_rejected", remote_temporarily_unavailable, operation);
}

function query_for_scope(scope: GitProviderDiscoveryScope) {
	if (scope._tag === "organization") {
		return organization_query;
	}

	return scope._tag === "search" ? search_query : account_query;
}

function query_arguments(input: GitHubCliRepositoryQuery) {
	const args = [
		"api",
		"graphql",
		"--hostname",
		input.host,
		"--method",
		"POST",
		"--raw-field",
		`query=${query_for_scope(input.scope)}`,
		"--field",
		`first=${input.page_size}`,
	];

	if (input.native_cursor !== undefined) {
		args.push("--raw-field", `after=${input.native_cursor}`);
	}

	if (input.scope._tag === "organization") {
		args.push("--raw-field", `organization=${input.scope.organization}`);
	}

	if (input.scope._tag === "search") {
		args.push("--raw-field", `search=${input.scope.query}`);
	}

	return args;
}

function repository_arguments(input: GitHubCliRepositoryInspection) {
	return [
		"api",
		"graphql",
		"--hostname",
		input.host,
		"--method",
		"POST",
		"--raw-field",
		`query=${repository_query}`,
		"--raw-field",
		`owner=${input.owner}`,
		"--raw-field",
		`name=${input.name}`,
	];
}

function pull_request_association_arguments(input: GitHubCliPullRequestRead) {
	return [
		"api",
		"graphql",
		"--hostname",
		input.host,
		"--method",
		"POST",
		"--raw-field",
		`query=${pull_request_association_query}`,
		"--raw-field",
		`owner=${input.owner}`,
		"--raw-field",
		`name=${input.name}`,
		"--raw-field",
		`branch=${input.selected_branch}`,
	];
}

function pull_request_detail_arguments(input: GitHubCliPullRequestTargetRead) {
	return [
		"api",
		"graphql",
		"--hostname",
		input.host,
		"--method",
		"POST",
		"--raw-field",
		`query=${pull_request_detail_query}`,
		"--raw-field",
		`owner=${input.owner}`,
		"--raw-field",
		`name=${input.name}`,
		"--field",
		`number=${input.pull_request_number}`,
	];
}

function check_failure_detail_arguments(input: GitHubCliCheckFailureDetailRead) {
	return [
		"api",
		"graphql",
		"--hostname",
		input.host,
		"--method",
		"POST",
		"--raw-field",
		`query=${check_failure_detail_query}`,
		"--raw-field",
		`owner=${input.owner}`,
		"--raw-field",
		`name=${input.name}`,
		"--field",
		`number=${input.pull_request_number}`,
		"--raw-field",
		`check=${input.check_native_id}`,
	];
}

function failed_job_log_arguments(input: {
	readonly attempt: number;
	readonly check_database_id: number;
	readonly host: string;
	readonly name: string;
	readonly owner: string;
	readonly workflow_database_id: number;
}) {
	return [
		"run",
		"view",
		`${input.workflow_database_id}`,
		"--repo",
		`${input.host}/${input.owner}/${input.name}`,
		"--attempt",
		`${input.attempt}`,
		"--job",
		`${input.check_database_id}`,
		"--log-failed",
	];
}

function strip_disallowed_text(value: string) {
	return value.replace(/[\p{Cc}\p{Cf}]/gu, (character) =>
		["\t", "\n", "\r"].includes(character) ? character : "",
	);
}

function truncate_utf8(value: string, maximum_bytes: number) {
	const encoded = Buffer.from(value, "utf8");

	if (encoded.byteLength <= maximum_bytes) {
		return { truncated: false, value };
	}

	let end = maximum_bytes;

	while (end > 0 && ((encoded[end] ?? 0) & 0xc0) === 0x80) {
		end -= 1;
	}

	return {
		truncated: true,
		value: new TextDecoder("utf-8", { fatal: true }).decode(encoded.subarray(0, end)),
	};
}

function canonical_text(value: string | null) {
	if (value === null) {
		return { _tag: "unavailable" } as const;
	}

	const normalized = strip_disallowed_text(value);

	if (normalized.trim().length === 0) {
		return { _tag: "unavailable" } as const;
	}

	const bounded = truncate_utf8(normalized, 4 * 1024);

	return {
		_tag: "available",
		truncated: bounded.truncated,
		untrusted_text: bounded.value,
	} as const;
}

interface GitHubCloneDestinationPin {
	readonly canonical_parent: string;
	readonly canonical_root: string;
	readonly parent_identity: FileIdentity;
	readonly root_identity: FileIdentity;
}

type GitHubCloneVerificationRunner = (
	args: ReadonlyArray<string>,
) => Effect.Effect<ProcessRunnerResult, GitHubCliError>;

function canonical_path_key(path_service: Path.Path, value: string) {
	const normalized = path_service.normalize(value);

	return process.platform === "win32" ? normalized.toLowerCase() : normalized;
}

function same_canonical_path(path_service: Path.Path, left: string, right: string) {
	return canonical_path_key(path_service, left) === canonical_path_key(path_service, right);
}

function clone_destination_matches_proof(
	path_service: Path.Path,
	pin: GitHubCloneDestinationPin,
	proof: GitProviderCloneDestinationProofValue,
) {
	return (
		same_canonical_path(path_service, pin.canonical_parent, proof.projects_root) &&
		same_canonical_path(path_service, pin.canonical_root, proof.canonical_root) &&
		pin.parent_identity.device.toString() === proof.projects_root_device &&
		pin.parent_identity.inode.toString() === proof.projects_root_inode &&
		pin.root_identity.device.toString() === proof.root_device &&
		pin.root_identity.inode.toString() === proof.root_inode
	);
}

function ReadCloneDestination(
	file_system: FileSystem.FileSystem,
	path_service: Path.Path,
	projects_root: string,
	destination_path: string,
	require_empty: boolean,
	reason: "invalid_destination" | "outcome_unknown",
) {
	const operation = "clone_repository" as const;

	return Effect.gen(function* () {
		if (!path_service.isAbsolute(projects_root) || !path_service.isAbsolute(destination_path)) {
			return yield* Effect.fail(api_error(reason, false, operation));
		}

		const requested_root = path_service.resolve(destination_path);
		const requested_projects_root = path_service.resolve(projects_root);
		const canonical_projects_root = yield* file_system.realPath(projects_root);
		const projects_root_info = yield* file_system.stat(canonical_projects_root);
		const canonical_root = yield* file_system.realPath(destination_path);
		const root_info = yield* file_system.stat(canonical_root);
		const canonical_parent = yield* file_system.realPath(path_service.dirname(canonical_root));
		const parent_info = yield* file_system.stat(canonical_parent);
		const entries = require_empty ? yield* file_system.readDirectory(canonical_root) : [];

		if (
			projects_root_info.type !== "Directory" ||
			root_info.type !== "Directory" ||
			parent_info.type !== "Directory" ||
			!same_canonical_path(path_service, requested_projects_root, canonical_projects_root) ||
			!same_canonical_path(path_service, canonical_parent, canonical_projects_root) ||
			!same_canonical_path(path_service, requested_root, canonical_root) ||
			!same_canonical_path(
				path_service,
				path_service.dirname(canonical_root),
				canonical_parent,
			) ||
			entries.length > 0
		) {
			return yield* Effect.fail(api_error(reason, false, operation));
		}

		const root_file = yield* file_system.open(canonical_root, { flag: "r" });
		const parent_file = yield* file_system.open(canonical_parent, { flag: "r" });
		const root_identity = yield* ReadFileIdentity(root_file.fd);
		const parent_identity = yield* ReadFileIdentity(parent_file.fd);

		return {
			canonical_parent,
			canonical_root,
			parent_identity,
			root_identity,
		} satisfies GitHubCloneDestinationPin;
	}).pipe(Effect.mapError(() => api_error(reason, false, operation)));
}

function VerifyCloneDestination(
	file_system: FileSystem.FileSystem,
	path_service: Path.Path,
	pin: GitHubCloneDestinationPin,
	require_empty: boolean,
	reason: "invalid_destination" | "outcome_unknown",
) {
	return ReadCloneDestination(
		file_system,
		path_service,
		pin.canonical_parent,
		pin.canonical_root,
		require_empty,
		reason,
	).pipe(
		Effect.filterOrFail(
			(current) =>
				same_file_identity(current.parent_identity, pin.parent_identity) &&
				same_file_identity(current.root_identity, pin.root_identity) &&
				same_canonical_path(path_service, current.canonical_parent, pin.canonical_parent) &&
				same_canonical_path(path_service, current.canonical_root, pin.canonical_root),
			() => api_error(reason, false, "clone_repository"),
		),
		Effect.asVoid,
	);
}

function exact_line(result: ProcessRunnerResult) {
	try {
		const value = new TextDecoder("utf-8", { fatal: true }).decode(result.stdout);
		const line = value.endsWith("\r\n")
			? value.slice(0, -2)
			: value.endsWith("\n")
				? value.slice(0, -1)
				: undefined;

		return line !== undefined &&
			line.length > 0 &&
			!line.includes("\0") &&
			!line.includes("\r") &&
			!line.includes("\n")
			? line
			: undefined;
	} catch {
		return undefined;
	}
}

function exact_nul_output(result: ProcessRunnerResult) {
	try {
		return new TextDecoder("utf-8", { fatal: true }).decode(result.stdout);
	} catch {
		return undefined;
	}
}

function single_worktree_path(result: ProcessRunnerResult) {
	const output = exact_nul_output(result);

	if (output === undefined || !output.endsWith("\0\0")) {
		return undefined;
	}

	const records = output
		.slice(0, -2)
		.split("\0\0")
		.map((record) => record.split("\0"));

	if (records.length !== 1 || records[0] === undefined || records[0].includes("bare")) {
		return undefined;
	}

	const worktree_fields = records[0].filter((field) => field.startsWith("worktree "));

	return worktree_fields.length === 1 ? worktree_fields[0]?.slice("worktree ".length) : undefined;
}

function VerifyClonedCheckout(
	file_system: FileSystem.FileSystem,
	path_service: Path.Path,
	RunGit: GitHubCloneVerificationRunner,
	pin: GitHubCloneDestinationPin,
	expected_url: string,
	receipt: string,
) {
	const Fail = () => Effect.fail(api_error("outcome_unknown", false, "clone_repository"));

	return Effect.gen(function* () {
		yield* VerifyCloneDestination(file_system, path_service, pin, false, "outcome_unknown");

		const top_level = exact_line(yield* RunGit(["rev-parse", "--show-toplevel"]));
		const inside_worktree = exact_line(yield* RunGit(["rev-parse", "--is-inside-work-tree"]));
		const bare_repository = exact_line(yield* RunGit(["rev-parse", "--is-bare-repository"]));

		if (top_level === undefined || inside_worktree !== "true" || bare_repository !== "false") {
			return yield* Fail();
		}

		const canonical_top_level = yield* file_system.realPath(top_level);
		const worktree_path = single_worktree_path(
			yield* RunGit(["worktree", "list", "--porcelain", "-z"]),
		);

		if (
			worktree_path === undefined ||
			!same_canonical_path(path_service, canonical_top_level, pin.canonical_root)
		) {
			return yield* Fail();
		}

		const canonical_worktree = yield* file_system.realPath(worktree_path);

		if (!same_canonical_path(path_service, canonical_worktree, pin.canonical_root)) {
			return yield* Fail();
		}

		const remotes = exact_line(yield* RunGit(["remote"]));
		const fetch_url = exact_line(yield* RunGit(["remote", "get-url", "--all", "origin"]));
		const push_url = exact_line(
			yield* RunGit(["remote", "get-url", "--all", "--push", "origin"]),
		);
		const origin_config = exact_nul_output(
			yield* RunGit([
				"config",
				"--local",
				"--null",
				"--no-includes",
				"--get-regexp",
				"^remote\\.origin\\.(url|pushurl)$",
			]),
		);

		if (
			remotes !== "origin" ||
			fetch_url !== expected_url ||
			push_url !== expected_url ||
			origin_config !== `remote.origin.url\n${expected_url}\0`
		) {
			return yield* Fail();
		}

		const git_directory = exact_line(yield* RunGit(["rev-parse", "--absolute-git-dir"]));

		if (git_directory === undefined) {
			return yield* Fail();
		}

		const canonical_git_directory = yield* file_system.realPath(git_directory);
		const expected_git_directory = yield* file_system.realPath(
			path_service.join(pin.canonical_root, ".git"),
		);
		const receipt_path = path_service.join(canonical_git_directory, clone_receipt_name);
		const observed_receipt = yield* file_system.readFileString(receipt_path);

		if (
			!same_canonical_path(path_service, canonical_git_directory, expected_git_directory) ||
			observed_receipt !== `${receipt}\n`
		) {
			return yield* Fail();
		}

		yield* VerifyCloneDestination(file_system, path_service, pin, false, "outcome_unknown");

		return pin.canonical_root;
	}).pipe(Effect.mapError(() => api_error("outcome_unknown", false, "clone_repository")));
}

/** Builds the exact HTTPS clone URL used by the pinned Git execution path. */
export function github_https_clone_url(input: {
	readonly host: string;
	readonly name: string;
	readonly owner: string;
}) {
	const owner = encodeURIComponent(input.owner);
	const name = encodeURIComponent(input.name);

	return `https://${input.host}/${owner}/${name}.git`;
}

function clone_arguments(
	input: GitHubCliCloneInput,
	template_path: string,
	destination_path: string,
) {
	return [
		"clone",
		`--template=${template_path}`,
		"--no-recurse-submodules",
		"--origin=origin",
		"--",
		github_https_clone_url(input),
		destination_path,
	];
}

function normalize_permission(
	value: (typeof GitHubRepositoryNode.Type)["viewerPermission"],
): GitHubCliRepository["viewer_permission"] {
	if (value === null) {
		return "unknown";
	}

	const permissions = {
		ADMIN: "admin",
		MAINTAIN: "maintain",
		READ: "read",
		TRIAGE: "triage",
		WRITE: "write",
	} as const;

	return permissions[value];
}

function normalize_visibility(
	value: (typeof GitHubRepositoryNode.Type)["visibility"],
): GitHubCliRepository["visibility"] {
	const visibilities = {
		INTERNAL: "internal",
		PRIVATE: "private",
		PUBLIC: "public",
	} as const;

	return visibilities[value];
}

function normalize_repository(repository: typeof GitHubRepositoryNode.Type): GitHubCliRepository {
	return {
		archived: repository.isArchived,
		...(repository.defaultBranchRef === null
			? {}
			: { default_branch: repository.defaultBranchRef.name }),
		name: repository.name,
		name_with_owner: repository.nameWithOwner,
		native_id: repository.id,
		owner: repository.owner.login,
		ssh_url: repository.sshUrl,
		updated_at: repository.updatedAt,
		viewer_permission: normalize_permission(repository.viewerPermission),
		visibility: normalize_visibility(repository.visibility),
		web_url: repository.url,
	};
}

function normalize_page(viewer_login: string, connection: typeof GitHubRepositoryConnection.Type) {
	return Effect.gen(function* () {
		if (connection.nodes.some((node) => node === null)) {
			return yield* Effect.fail(api_error("invalid_response", false));
		}

		const repositories = connection.nodes.map((node) => normalize_repository(node!));
		const continuation = connection.pageInfo.hasNextPage
			? connection.pageInfo.endCursor === null || connection.pageInfo.endCursor.length === 0
				? undefined
				: ({
						cursor: connection.pageInfo.endCursor,
						type: "more",
					} as const)
			: ({ type: "complete" } as const);

		if (continuation === undefined) {
			return yield* Effect.fail(api_error("invalid_response", false));
		}

		return {
			continuation,
			repositories,
			viewer_login,
		} satisfies GitHubCliRepositoryPage;
	});
}

function DecodeRepositoryPage(scope: GitProviderDiscoveryScope, data: unknown) {
	if (scope._tag === "organization") {
		return ParseSchema(GitHubOrganizationRepositoryData, data).pipe(
			Effect.mapError(() => api_error("invalid_response", false)),
			Effect.flatMap((decoded) =>
				decoded.organization === null
					? Effect.fail(api_error("remote_not_found", false))
					: normalize_page(decoded.viewer.login, decoded.organization.repositories),
			),
		);
	}

	if (scope._tag === "search") {
		return ParseSchema(GitHubSearchRepositoryData, data).pipe(
			Effect.mapError(() => api_error("invalid_response", false)),
			Effect.flatMap((decoded) => normalize_page(decoded.viewer.login, decoded.search)),
		);
	}

	return ParseSchema(GitHubAccountRepositoryData, data).pipe(
		Effect.mapError(() => api_error("invalid_response", false)),
		Effect.flatMap((decoded) =>
			normalize_page(decoded.viewer.login, decoded.viewer.repositories),
		),
	);
}

function connection_is_consistent(connection: {
	readonly nodes: ReadonlyArray<unknown>;
	readonly pageInfo: typeof GitHubPageInfo.Type;
	readonly totalCount: number;
}) {
	return (
		connection.totalCount >= connection.nodes.length &&
		connection.pageInfo.hasNextPage === connection.totalCount > connection.nodes.length &&
		(!connection.pageInfo.hasNextPage ||
			(connection.pageInfo.endCursor !== null && connection.pageInfo.endCursor.length > 0))
	);
}

function pull_request_url_matches_host(value: string, host: string) {
	const parsed = URL.parse(value);

	return (
		parsed !== null &&
		(parsed.protocol === "https:" || parsed.protocol === "http:") &&
		parsed.host === host &&
		parsed.username === "" &&
		parsed.password === ""
	);
}

function valid_timeout(value: number) {
	return Number.isSafeInteger(value) && value > 0 && value <= 5 * 60_000;
}

function valid_clone_timeout(value: number) {
	return Number.isSafeInteger(value) && value > 0 && value <= 24 * 60 * 60_000;
}

/** Builds the dedicated GitHub CLI capability over the shared bounded process runner. */
export function make_github_cli_layer(options: GitHubCliOptions) {
	const command = options.command ?? "gh";
	const clone_timeout_ms = options.clone_timeout_ms ?? 30 * 60_000;
	const probe_timeout_ms = options.probe_timeout_ms ?? 10_000;
	const request_timeout_ms = options.request_timeout_ms ?? 30_000;

	return Layer.effect(
		GitHubCli,
		Effect.gen(function* () {
			if (!valid_clone_timeout(clone_timeout_ms)) {
				return yield* Effect.fail(
					new GitHubCliConfigurationError({ field: "clone_timeout_ms" }),
				);
			}

			if (!valid_timeout(probe_timeout_ms)) {
				return yield* Effect.fail(
					new GitHubCliConfigurationError({ field: "probe_timeout_ms" }),
				);
			}

			if (!valid_timeout(request_timeout_ms)) {
				return yield* Effect.fail(
					new GitHubCliConfigurationError({ field: "request_timeout_ms" }),
				);
			}

			const executable = yield* GitHubCliExecutable;
			const git_executable = yield* GitHubCliGitExecutable;
			const file_system = yield* FileSystem.FileSystem;
			const path_service = yield* Path.Path;
			const runner = yield* ProcessRunner;
			const crypto = yield* Crypto.Crypto;
			const environment = selected_environment();
			const selected_gh_config_directory = github_cli_config_directory(
				environment,
				path_service,
				options.cwd,
			);
			const Run = (
				executable_path: string,
				args: ReadonlyArray<string>,
				max_stdout_bytes: number,
				timeout_ms: number,
				environment_override: Readonly<Record<string, string | undefined>> = {},
				cwd = options.cwd,
			) =>
				runner
					.Run({
						args,
						command: executable_path,
						cwd,
						environment: { ...environment, ...environment_override },
						environment_mode: "replace",
						max_stderr_bytes: 256 * 1024,
						max_stdout_bytes,
					})
					.pipe(Effect.timeoutOption(timeout_ms));
			const Inspect = Effect.gen(function* () {
				const location = yield* executable.Locate;

				if (Option.isNone(location)) {
					return { command, type: "missing" as const };
				}

				const executable_path = location.value.path;
				const version_result = yield* Run(
					executable_path,
					["version"],
					16 * 1024,
					probe_timeout_ms,
				).pipe(Effect.option);

				if (Option.isNone(version_result)) {
					return {
						command,
						executable_path,
						reason: "process_failed" as const,
						type: "unavailable" as const,
					};
				}

				if (Option.isNone(version_result.value)) {
					return {
						command,
						executable_path,
						reason: "timed_out" as const,
						type: "unavailable" as const,
					};
				}

				const version_process = version_result.value.value;
				const version = parse_version(version_process.stdout);

				if (
					version_process.exit_code !== 0 ||
					version_process.stdout_truncated ||
					version === undefined
				) {
					return {
						command,
						executable_path,
						reason: "invalid_output" as const,
						type: "unavailable" as const,
					};
				}

				const auth_result = yield* Run(
					executable_path,
					["auth", "status", "--json", "hosts"],
					512 * 1024,
					probe_timeout_ms,
				).pipe(Effect.option);

				if (Option.isNone(auth_result)) {
					return {
						command,
						executable_path,
						reason: "process_failed" as const,
						type: "unavailable" as const,
						version,
					};
				}

				if (Option.isNone(auth_result.value)) {
					return {
						command,
						executable_path,
						reason: "timed_out" as const,
						type: "unavailable" as const,
						version,
					};
				}

				const auth_process = auth_result.value.value;

				if (auth_process.exit_code !== 0) {
					return incompatible_auth_status(auth_process)
						? {
								executable_path,
								reason: "required_features_missing" as const,
								type: "incompatible" as const,
								version,
							}
						: {
								command,
								executable_path,
								reason: "auth_probe_failed" as const,
								type: "unavailable" as const,
								version,
							};
				}

				if (auth_process.stdout_truncated) {
					return {
						command,
						executable_path,
						reason: "invalid_output" as const,
						type: "unavailable" as const,
						version,
					};
				}

				const status = yield* ParseAuthStatus(auth_process.stdout);

				if (Option.isNone(status)) {
					return {
						command,
						executable_path,
						reason: "invalid_output" as const,
						type: "unavailable" as const,
						version,
					};
				}

				const host_entries = Object.entries(status.value.hosts);

				if (
					host_entries.some(([host, accounts]) =>
						accounts.some((account) => account.host !== host),
					)
				) {
					return {
						command,
						executable_path,
						reason: "invalid_output" as const,
						type: "unavailable" as const,
						version,
					};
				}

				const hosts = host_entries.map(([host, accounts]) => ({
					accounts: normalize_accounts(host, accounts),
					host,
				}));

				return {
					executable_path,
					hosts,
					type: "available" as const,
					version,
				};
			}).pipe(
				Effect.catch(() =>
					Effect.succeed({
						command,
						reason: "process_failed",
						type: "unavailable",
					} as const),
				),
			);
			const QueryRepositories = (input: GitHubCliRepositoryQuery) =>
				Effect.gen(function* () {
					const location = yield* executable.Locate;

					if (Option.isNone(location)) {
						return yield* Effect.fail(api_error("dependency_missing", false));
					}

					const process_option = yield* Run(
						location.value.path,
						query_arguments(input),
						2 * 1024 * 1024,
						request_timeout_ms,
					).pipe(Effect.mapError(() => api_error("process_failed", true)));

					if (Option.isNone(process_option)) {
						return yield* Effect.fail(api_error("network_unavailable", true));
					}

					const result = process_option.value;

					if (result.stdout_truncated || result.stderr_truncated) {
						return yield* Effect.fail(api_error("invalid_response", false));
					}

					const envelope = yield* ParseEnvelope(result.stdout);

					if (
						result.exit_code !== 0 ||
						(Option.isSome(envelope) && (envelope.value.errors?.length ?? 0) > 0)
					) {
						return yield* Effect.fail(classify_api_failure(result, envelope));
					}

					if (
						Option.isNone(envelope) ||
						envelope.value.data === undefined ||
						envelope.value.data === null
					) {
						return yield* Effect.fail(api_error("invalid_response", false));
					}

					return yield* DecodeRepositoryPage(input.scope, envelope.value.data);
				});
			const InspectRepository = (input: GitHubCliRepositoryInspection) =>
				Effect.gen(function* () {
					const location = yield* executable.Locate;
					const operation = "inspect_repository" as const;

					if (Option.isNone(location)) {
						return yield* Effect.fail(
							api_error("dependency_missing", false, operation),
						);
					}

					const process_option = yield* Run(
						location.value.path,
						repository_arguments(input),
						2 * 1024 * 1024,
						request_timeout_ms,
						{ GH_HOST: input.host },
					).pipe(Effect.mapError(() => api_error("process_failed", true, operation)));

					if (Option.isNone(process_option)) {
						return yield* Effect.fail(api_error("timed_out", true, operation));
					}

					const result = process_option.value;

					if (result.stdout_truncated || result.stderr_truncated) {
						return yield* Effect.fail(api_error("invalid_response", false, operation));
					}

					const envelope = yield* ParseEnvelope(result.stdout);

					if (
						result.exit_code !== 0 ||
						(Option.isSome(envelope) && (envelope.value.errors?.length ?? 0) > 0)
					) {
						return yield* Effect.fail(
							classify_api_failure(result, envelope, operation),
						);
					}

					if (
						Option.isNone(envelope) ||
						envelope.value.data === undefined ||
						envelope.value.data === null
					) {
						return yield* Effect.fail(api_error("invalid_response", false, operation));
					}

					const decoded = yield* ParseSchema(
						GitHubSingleRepositoryData,
						envelope.value.data,
					).pipe(Effect.mapError(() => api_error("invalid_response", false, operation)));

					if (decoded.repository === null) {
						return yield* Effect.fail(api_error("remote_not_found", false, operation));
					}

					return {
						repository: normalize_repository(decoded.repository),
						viewer_login: decoded.viewer.login,
					} satisfies GitHubCliRepositoryInspectionResult;
				});
			const ReadPullRequestDetail = (
				input: GitHubCliPullRequestTargetRead,
				operation: Extract<
					GitHubCliError["operation"],
					"read_pull_request" | "read_pull_request_target"
				>,
			) =>
				Effect.gen(function* () {
					const location = yield* executable.Locate;

					if (Option.isNone(location)) {
						return yield* Effect.fail(
							api_error("dependency_missing", false, operation),
						);
					}

					const process_option = yield* Run(
						location.value.path,
						pull_request_detail_arguments(input),
						2 * 1024 * 1024,
						request_timeout_ms,
						{ GH_HOST: input.host },
					).pipe(Effect.mapError(() => api_error("process_failed", true, operation)));

					if (Option.isNone(process_option)) {
						return yield* Effect.fail(api_error("timed_out", true, operation));
					}

					const result = process_option.value;

					if (result.stdout_truncated || result.stderr_truncated) {
						return yield* Effect.fail(api_error("invalid_response", false, operation));
					}

					const envelope = yield* ParseEnvelope(result.stdout);

					if (
						result.exit_code !== 0 ||
						(Option.isSome(envelope) && (envelope.value.errors?.length ?? 0) > 0)
					) {
						return yield* Effect.fail(
							classify_api_failure(result, envelope, operation),
						);
					}

					if (
						Option.isNone(envelope) ||
						envelope.value.data === undefined ||
						envelope.value.data === null
					) {
						return yield* Effect.fail(api_error("invalid_response", false, operation));
					}

					const detail = yield* ParseStrictSchema(
						GitHubPullRequestDetailData,
						envelope.value.data,
					).pipe(Effect.mapError(() => api_error("invalid_response", false, operation)));

					if (detail.repository === null || detail.repository.pullRequest === null) {
						return yield* Effect.fail(api_error("remote_not_found", false, operation));
					}

					const pull_request = detail.repository.pullRequest;

					if (
						pull_request.number !== input.pull_request_number ||
						pull_request.id !== input.pull_request_native_id ||
						pull_request.headRefName !== input.selected_branch ||
						pull_request.headRepository === null ||
						pull_request.headRepository.name.toLowerCase() !==
							input.name.toLowerCase() ||
						pull_request.headRepository.owner.login.toLowerCase() !==
							input.owner.toLowerCase() ||
						pull_request.isMerged !== (pull_request.state === "MERGED") ||
						!pull_request_url_matches_host(pull_request.url, input.host)
					) {
						return yield* Effect.fail(api_error("invalid_response", false, operation));
					}

					const rollup = pull_request.commits.nodes[0]!.commit.statusCheckRollup;
					const invalid_comments = pull_request.reviewThreads.nodes.some(
						(thread) =>
							thread.comments.nodes.length !==
							Math.min(thread.comments.totalCount, 1),
					);
					const invalid_annotations =
						rollup?.contexts.nodes.some(
							(check) =>
								check.__typename === "CheckRun" &&
								check.annotations !== null &&
								!connection_is_consistent(check.annotations),
						) ?? false;

					if (
						!connection_is_consistent(pull_request.reviews) ||
						!connection_is_consistent(pull_request.reviewThreads) ||
						!connection_is_consistent(pull_request.requestedReviewers) ||
						(rollup !== null && !connection_is_consistent(rollup.contexts)) ||
						invalid_comments ||
						invalid_annotations
					) {
						return yield* Effect.fail(api_error("invalid_response", false, operation));
					}

					return {
						pull_request,
						type: "matched_pull_request",
						viewer_login: detail.viewer.login,
					} as const;
				});
			const ReadPullRequest = (input: GitHubCliPullRequestRead) =>
				Effect.gen(function* () {
					const location = yield* executable.Locate;
					const operation = "read_pull_request" as const;

					if (Option.isNone(location)) {
						return yield* Effect.fail(
							api_error("dependency_missing", false, operation),
						);
					}

					const Execute = (args: ReadonlyArray<string>) =>
						Run(location.value.path, args, 2 * 1024 * 1024, request_timeout_ms, {
							GH_HOST: input.host,
						}).pipe(
							Effect.mapError(() => api_error("process_failed", true, operation)),
						);
					const association_process = yield* Execute(
						pull_request_association_arguments(input),
					);

					if (Option.isNone(association_process)) {
						return yield* Effect.fail(api_error("timed_out", true, operation));
					}

					const association_result = association_process.value;

					if (
						association_result.stdout_truncated ||
						association_result.stderr_truncated
					) {
						return yield* Effect.fail(api_error("invalid_response", false, operation));
					}

					const association_envelope = yield* ParseEnvelope(association_result.stdout);

					if (
						association_result.exit_code !== 0 ||
						(Option.isSome(association_envelope) &&
							(association_envelope.value.errors?.length ?? 0) > 0)
					) {
						return yield* Effect.fail(
							classify_api_failure(
								association_result,
								association_envelope,
								operation,
							),
						);
					}

					if (
						Option.isNone(association_envelope) ||
						association_envelope.value.data === undefined ||
						association_envelope.value.data === null
					) {
						return yield* Effect.fail(api_error("invalid_response", false, operation));
					}

					const association = yield* ParseStrictSchema(
						GitHubPullRequestAssociationData,
						association_envelope.value.data,
					).pipe(Effect.mapError(() => api_error("invalid_response", false, operation)));

					if (association.repository === null) {
						return yield* Effect.fail(api_error("remote_not_found", false, operation));
					}

					const observed_candidates = association.repository.pullRequests.nodes;
					const candidates = observed_candidates.filter(
						(candidate) =>
							candidate.headRepository !== null &&
							candidate.headRepository.name.toLowerCase() ===
								input.name.toLowerCase() &&
							candidate.headRepository.owner.login.toLowerCase() ===
								input.owner.toLowerCase(),
					);
					const candidate_numbers = candidates.map((candidate) => candidate.number);
					const association_page = association.repository.pullRequests.pageInfo;

					if (
						observed_candidates.some(
							(candidate) =>
								candidate.headRefName !== input.selected_branch ||
								candidate.isMerged !== (candidate.state === "MERGED") ||
								!pull_request_url_matches_host(candidate.url, input.host),
						) ||
						new Set(candidate_numbers).size !== candidate_numbers.length ||
						(association_page.hasNextPage &&
							(observed_candidates.length !== 10 ||
								association_page.endCursor === null ||
								association_page.endCursor.length === 0)) ||
						(association_page.hasNextPage && candidates.length < 2)
					) {
						return yield* Effect.fail(api_error("invalid_response", false, operation));
					}

					if (candidates.length === 0) {
						return {
							type: "no_pull_request",
							viewer_login: association.viewer.login,
						} as const;
					}

					if (
						candidates.length > 1 ||
						association.repository.pullRequests.pageInfo.hasNextPage
					) {
						return {
							candidates,
							complete: !association.repository.pullRequests.pageInfo.hasNextPage,
							type: "ambiguous_pull_requests",
							viewer_login: association.viewer.login,
						} as const;
					}

					const detail = yield* ReadPullRequestDetail(
						{
							...input,
							pull_request_native_id: candidates[0]!.id,
							pull_request_number: candidates[0]!.number,
						},
						operation,
					);

					if (
						detail.pull_request.id !== candidates[0]!.id ||
						detail.pull_request.headRefOid !== candidates[0]!.headRefOid
					) {
						return yield* Effect.fail(api_error("invalid_response", false, operation));
					}

					if (
						detail.viewer_login.toLowerCase() !== association.viewer.login.toLowerCase()
					) {
						return yield* Effect.fail(
							api_error("authentication_required", false, operation),
						);
					}

					return {
						pull_request: detail.pull_request,
						type: "matched_pull_request",
						viewer_login: detail.viewer_login,
					} as const;
				});
			const ReadPullRequestTarget = (input: GitHubCliPullRequestTargetRead) =>
				ReadPullRequestDetail(input, "read_pull_request_target");
			const ReadCheckFailureDetail = (input: GitHubCliCheckFailureDetailRead) =>
				Effect.gen(function* () {
					const location = yield* executable.Locate;
					const operation = "read_check_failure_detail" as const;

					if (Option.isNone(location)) {
						return yield* Effect.fail(
							api_error("dependency_missing", false, operation),
						);
					}

					const process_option = yield* Run(
						location.value.path,
						check_failure_detail_arguments(input),
						2 * 1024 * 1024,
						request_timeout_ms,
						{ GH_HOST: input.host },
					).pipe(Effect.mapError(() => api_error("process_failed", true, operation)));

					if (Option.isNone(process_option)) {
						return yield* Effect.fail(api_error("timed_out", true, operation));
					}

					const result = process_option.value;

					if (result.stdout_truncated || result.stderr_truncated) {
						return yield* Effect.fail(api_error("invalid_response", false, operation));
					}

					const envelope = yield* ParseEnvelope(result.stdout);

					if (
						result.exit_code !== 0 ||
						(Option.isSome(envelope) && (envelope.value.errors?.length ?? 0) > 0)
					) {
						return yield* Effect.fail(
							classify_api_failure(result, envelope, operation),
						);
					}

					if (
						Option.isNone(envelope) ||
						envelope.value.data === undefined ||
						envelope.value.data === null
					) {
						return yield* Effect.fail(api_error("invalid_response", false, operation));
					}

					const detail = yield* ParseStrictSchema(
						GitHubCheckFailureDetailData,
						envelope.value.data,
					).pipe(Effect.mapError(() => api_error("invalid_response", false, operation)));
					const repository = detail.repository;
					const pull_request = repository?.pullRequest;
					const check = detail.checkRun;

					if (
						repository === null ||
						repository === undefined ||
						pull_request === null ||
						pull_request === undefined ||
						check === null ||
						repository.nameWithOwner.toLowerCase() !==
							`${input.owner}/${input.name}`.toLowerCase() ||
						pull_request.id !== input.pull_request_native_id ||
						pull_request.number !== input.pull_request_number ||
						pull_request.headRefName !== input.selected_branch ||
						pull_request.headRefOid !== input.expected_head ||
						pull_request.headRepository === null ||
						pull_request.headRepository.name.toLowerCase() !==
							input.name.toLowerCase() ||
						pull_request.headRepository.owner.login.toLowerCase() !==
							input.owner.toLowerCase() ||
						check.id !== input.check_native_id ||
						check.checkSuite.commit.oid !== input.expected_head ||
						check.checkSuite.commit.repository === null ||
						check.checkSuite.commit.repository.id !== repository.id ||
						check.checkSuite.commit.repository.nameWithOwner.toLowerCase() !==
							repository.nameWithOwner.toLowerCase()
					) {
						return yield* Effect.fail(api_error("invalid_response", false, operation));
					}

					const sanitized_title =
						check.title === null
							? undefined
							: truncate_utf8(strip_disallowed_text(check.title), 1024).value;
					const output = {
						summary: canonical_text(check.summary),
						text: canonical_text(check.text),
						...(sanitized_title === undefined || sanitized_title.trim().length === 0
							? {}
							: { title: sanitized_title }),
					};
					const workflow_run = check.checkSuite.workflowRun;

					if (check.completedAt === null || check.status !== "COMPLETED") {
						return {
							check_native_id: check.id,
							head_commit: check.checkSuite.commit.oid,
							log: { _tag: "unavailable", reason: "check_not_completed" },
							name: check.name,
							output,
							viewer_login: detail.viewer.login,
						} satisfies GitHubCliCheckFailureDetail;
					}

					if (
						workflow_run === null ||
						workflow_run.databaseId === null ||
						check.databaseId === null
					) {
						return {
							check_native_id: check.id,
							head_commit: check.checkSuite.commit.oid,
							log: {
								_tag: "unavailable",
								reason: workflow_run === null ? "not_actions_job" : "not_available",
							},
							name: check.name,
							output,
							viewer_login: detail.viewer.login,
							...(workflow_run === null
								? {}
								: {
										attempt: workflow_run.runAttempt,
										workflow_native_id: workflow_run.id,
									}),
						} satisfies GitHubCliCheckFailureDetail;
					}

					const log_process = yield* Run(
						location.value.path,
						failed_job_log_arguments({
							attempt: workflow_run.runAttempt,
							check_database_id: check.databaseId,
							host: input.host,
							name: input.name,
							owner: input.owner,
							workflow_database_id: workflow_run.databaseId,
						}),
						64 * 1024,
						request_timeout_ms,
						{ GH_HOST: input.host },
					).pipe(Effect.mapError(() => api_error("process_failed", true, operation)));

					if (Option.isNone(log_process)) {
						return yield* Effect.fail(api_error("timed_out", true, operation));
					}

					const log_result = log_process.value;

					if (log_result.exit_code !== 0) {
						const failure = classify_api_failure(log_result, Option.none(), operation);

						if (
							failure.reason !== "remote_not_found" &&
							(failure.reason !== "remote_rejected" || failure.retryable)
						) {
							return yield* Effect.fail(failure);
						}

						return {
							attempt: workflow_run.runAttempt,
							check_native_id: check.id,
							head_commit: check.checkSuite.commit.oid,
							log: { _tag: "unavailable", reason: "not_available" },
							name: check.name,
							output,
							viewer_login: detail.viewer.login,
							workflow_native_id: workflow_run.id,
						} satisfies GitHubCliCheckFailureDetail;
					}

					const excerpt = truncate_utf8(
						strip_disallowed_text(decode_text(log_result.stdout)),
						64 * 1024,
					);
					const log =
						excerpt.value.trim().length === 0
							? ({ _tag: "unavailable", reason: "not_available" } as const)
							: ({
									_tag: "available" as const,
									observed_bytes: log_result.stdout_bytes,
									truncated: log_result.stdout_truncated || excerpt.truncated,
									untrusted_excerpt: excerpt.value,
								} as const);

					return {
						attempt: workflow_run.runAttempt,
						check_native_id: check.id,
						head_commit: check.checkSuite.commit.oid,
						log,
						name: check.name,
						output,
						viewer_login: detail.viewer.login,
						workflow_native_id: workflow_run.id,
					} satisfies GitHubCliCheckFailureDetail;
				});
			const CloneRepository = (input: GitHubCliCloneInput) =>
				Effect.gen(function* () {
					const location = yield* executable.Locate;
					const git_location = yield* git_executable.Locate;
					const operation = "clone_repository" as const;
					const destination = yield* Schema.decodeUnknownEffect(
						GitProviderCloneDestinationProof,
						{ onExcessProperty: "error" },
					)(input.destination).pipe(
						Effect.mapError(() => api_error("invalid_destination", false, operation)),
					);

					if (Option.isNone(location)) {
						return yield* Effect.fail(
							api_error("dependency_missing", false, operation),
						);
					}

					if (Option.isNone(git_location)) {
						return yield* Effect.fail(
							api_error("git_dependency_missing", false, operation),
						);
					}

					if (options.projects_root === undefined) {
						return yield* Effect.fail(
							api_error("invalid_destination", false, operation),
						);
					}

					if (selected_gh_config_directory === undefined) {
						return yield* Effect.fail(
							api_error("authentication_required", false, operation),
						);
					}

					const git_path = git_location.value.path;
					const expected_url = github_https_clone_url(input);
					const projects_root = options.projects_root;

					return yield* Effect.gen(function* () {
						const pin = yield* ReadCloneDestination(
							file_system,
							path_service,
							projects_root,
							destination.canonical_root,
							true,
							"invalid_destination",
						);

						if (!clone_destination_matches_proof(path_service, pin, destination)) {
							return yield* Effect.fail(
								api_error("invalid_destination", false, operation),
							);
						}

						const template_path = yield* file_system.makeTempDirectoryScoped({
							prefix: "artisan-github-clone-template-",
						});
						const receipt = yield* crypto.randomUUIDv4;
						const receipt_path = path_service.join(template_path, clone_receipt_name);

						yield* file_system.writeFileString(receipt_path, `${receipt}\n`, {
							flag: "wx",
							mode: 0o600,
						});

						return yield* Effect.uninterruptibleMask((restore) =>
							restore(
								Effect.gen(function* () {
									yield* VerifyCloneDestination(
										file_system,
										path_service,
										pin,
										true,
										"outcome_unknown",
									);

									const process_option = yield* Run(
										git_path,
										clone_arguments(input, template_path, pin.canonical_root),
										64 * 1024,
										clone_timeout_ms,
										clone_environment(
											input,
											location.value.path,
											git_path,
											path_service,
											template_path,
											selected_gh_config_directory,
										),
										pin.canonical_parent,
									).pipe(
										Effect.mapError(() =>
											api_error("outcome_unknown", false, operation),
										),
									);

									if (Option.isNone(process_option)) {
										return yield* Effect.fail(
											api_error("outcome_unknown", false, operation),
										);
									}

									const result = process_option.value;

									if (result.exit_code !== 0) {
										return yield* Effect.fail(
											api_error("outcome_unknown", false, operation),
										);
									}

									const RunGit: GitHubCloneVerificationRunner = (args) =>
										Run(
											git_path,
											[
												"-c",
												`core.hooksPath=${null_device_path}`,
												"-c",
												"credential.helper=",
												"-C",
												pin.canonical_root,
												...args,
											],
											64 * 1024,
											request_timeout_ms,
											{ PATH: path_service.dirname(git_path) },
											pin.canonical_parent,
										).pipe(
											Effect.mapError(() =>
												api_error("outcome_unknown", false, operation),
											),
											Effect.flatMap((option) =>
												Option.match(option, {
													onNone: () =>
														Effect.fail(
															api_error(
																"outcome_unknown",
																false,
																operation,
															),
														),
													onSome: (verification) =>
														verification.exit_code === 0 &&
														!verification.stdout_truncated &&
														!verification.stderr_truncated
															? Effect.succeed(verification)
															: Effect.fail(
																	api_error(
																		"outcome_unknown",
																		false,
																		operation,
																	),
																),
												}),
											),
										);
									const canonical_root = yield* VerifyClonedCheckout(
										file_system,
										path_service,
										RunGit,
										pin,
										expected_url,
										receipt,
									);
									const VerifyCheckout = VerifyClonedCheckout(
										file_system,
										path_service,
										RunGit,
										pin,
										expected_url,
										receipt,
									).pipe(Effect.asVoid);

									return {
										VerifyCheckout,
										canonical_root,
										output_complete:
											!result.stdout_truncated && !result.stderr_truncated,
									} satisfies GitHubCliCloneResult;
								}),
							).pipe(
								Effect.matchCauseEffect({
									onFailure: (cause) =>
										Cause.hasInterrupts(cause)
											? Effect.fail(
													api_error("outcome_unknown", false, operation),
												)
											: Effect.failCause(cause),
									onSuccess: Effect.succeed,
								}),
							),
						);
					}).pipe(
						Effect.mapError((cause) =>
							cause instanceof GitHubCliError
								? cause
								: api_error("process_failed", false, operation),
						),
					);
				});

			return {
				CloneRepository,
				Inspect,
				InspectRepository,
				QueryRepositories,
				ReadCheckFailureDetail,
				ReadPullRequest,
				ReadPullRequestTarget,
			};
		}),
	);
}
