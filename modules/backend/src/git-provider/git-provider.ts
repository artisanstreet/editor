import { Context, Data, Effect, Schema } from "effect";
import { GitBranchName, GitObjectId, HostedGitPullRequestLookup } from "@artisan/protocol";

const text_encoder = new TextEncoder();

const BoundedVisibleString = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.trim().length === 0 ||
		text_encoder.encode(value).byteLength > 512 ||
		/[\p{Cc}]/u.test(value)
			? "Expected a non-empty bounded visible string without control characters"
			: undefined,
	),
);

export const GitProviderId = Schema.String.check(
	Schema.isPattern(/^[a-z][a-z0-9_-]{0,63}$/, {
		message: "Expected a lowercase bounded Git provider ID",
	}),
);

export type GitProviderId = typeof GitProviderId.Type;

export const GitProviderHost = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		normalize_git_provider_host(value) === value
			? undefined
			: "Expected a canonical Git provider host",
	),
);

export type GitProviderHost = typeof GitProviderHost.Type;

const GitProviderExecutablePath = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.length === 0 ||
		text_encoder.encode(value).byteLength > 4_096 ||
		/[\p{Cc}]/u.test(value)
			? "Expected a bounded executable path without control characters"
			: undefined,
	),
);

export const GitProviderNativePath = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.length === 0 ||
		text_encoder.encode(value).byteLength > 4_096 ||
		value.includes("\0") ||
		/[\r\n]/u.test(value) ||
		!(/^[A-Za-z]:[\\/]/u.test(value) || value.startsWith("/") || value.startsWith("\\\\"))
			? "Expected a bounded absolute native path without NUL or line breaks"
			: undefined,
	),
);

export type GitProviderNativePath = typeof GitProviderNativePath.Type;

const GitProviderFileIdentityPart = Schema.String.check(
	Schema.isPattern(/^(?:0|[1-9][0-9]*)$/u, {
		message: "Expected an unsigned file identity component",
	}),
);

/** Binds one visible empty clone destination to its approved filesystem identity. */
export const GitProviderCloneDestinationProof = Schema.Struct({
	canonical_root: GitProviderNativePath,
	projects_root: GitProviderNativePath,
	projects_root_device: GitProviderFileIdentityPart,
	projects_root_inode: GitProviderFileIdentityPart,
	root_device: GitProviderFileIdentityPart,
	root_inode: GitProviderFileIdentityPart,
});

export type GitProviderCloneDestinationProof = typeof GitProviderCloneDestinationProof.Type;

function is_git_provider_url(value: string, protocols: ReadonlyArray<string>) {
	if (text_encoder.encode(value).byteLength > 2_048 || /[\p{Cc}\p{Cf}]/u.test(value)) {
		return false;
	}

	if (
		protocols.includes("ssh:") &&
		/^[A-Za-z0-9._-]+@[A-Za-z0-9.-]+:[A-Za-z0-9._~/-]+$/u.test(value)
	) {
		return true;
	}

	if (!URL.canParse(value)) {
		return false;
	}

	const parsed = URL.parse(value);

	return (
		parsed !== null &&
		protocols.includes(parsed.protocol) &&
		parsed.hostname.length > 0 &&
		parsed.password === "" &&
		parsed.hash === "" &&
		parsed.search === "" &&
		(parsed.protocol === "ssh:" || parsed.username === "")
	);
}

/** Validates a bounded clone URL using HTTP(S) or SSH without embedded credentials. */
export const GitProviderUrl = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		is_git_provider_url(value, ["http:", "https:", "ssh:"])
			? undefined
			: "Expected a bounded HTTP(S) or SSH URL without credentials or fragments",
	),
);

export type GitProviderUrl = typeof GitProviderUrl.Type;

/** Validates a bounded browser URL without credentials or fragments. */
export const GitProviderWebUrl = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		is_git_provider_url(value, ["http:", "https:"])
			? undefined
			: "Expected a bounded HTTP(S) URL without credentials or fragments",
	),
);

export type GitProviderWebUrl = typeof GitProviderWebUrl.Type;

const GitProviderCursor = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.length === 0 ||
		text_encoder.encode(value).byteLength > 2_048 ||
		/[\p{Cc}]/u.test(value)
			? "Expected a bounded cursor without control characters"
			: undefined,
	),
);

const GitProviderQuery = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.trim().length === 0 ||
		text_encoder.encode(value).byteLength > 256 ||
		/[\p{Cc}]/u.test(value)
			? "Expected a bounded repository query without control characters"
			: undefined,
	),
);

const GitProviderAccountLogin = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.trim().length === 0 ||
		text_encoder.encode(value).byteLength > 128 ||
		/[\p{Cc}\s]/u.test(value)
			? "Expected a bounded account login without whitespace or control characters"
			: undefined,
	),
);

/** Normalizes a host[:port] input without accepting a URL or remote path. */
export function normalize_git_provider_host(input: string): string | undefined {
	const is_host_port = /^(?:\[[0-9A-Fa-f:.]+\]|[^:/?#@\s]+)(?::[0-9]{1,5})?$/.test(input);
	const candidate = `https://${input}`;

	if (
		input.length === 0 ||
		input !== input.trim() ||
		/[\p{Cc}\p{Cf}]/u.test(input) ||
		!is_host_port ||
		!URL.canParse(candidate)
	) {
		return undefined;
	}

	const parsed = URL.parse(candidate);

	if (
		parsed === null ||
		parsed.protocol !== "https:" ||
		parsed.username !== "" ||
		parsed.password !== "" ||
		parsed.pathname !== "/" ||
		parsed.search !== "" ||
		parsed.hash !== "" ||
		parsed.hostname.length === 0
	) {
		return undefined;
	}

	return parsed.port === "" ? parsed.hostname : `${parsed.hostname}:${parsed.port}`;
}

/** Identifies the provider-owned capability exposed through a canonical projection. */
export const GitProviderCapabilityKind = Schema.Literals([
	"clone_repository",
	"discover_repositories",
	"inspect_authentication",
	"read_ci",
	"read_reviews",
	"write_provider_mutations",
]);

export type GitProviderCapabilityKind = typeof GitProviderCapabilityKind.Type;

/** Describes a provider capability and whether the installed adapter can offer it. */
export const GitProviderCapability = Schema.Union([
	Schema.Struct({
		_tag: Schema.Literal("available"),
		capability: GitProviderCapabilityKind,
	}),
	Schema.Struct({
		_tag: Schema.Literal("unavailable"),
		capability: GitProviderCapabilityKind,
		reason: BoundedVisibleString,
	}),
	Schema.Struct({
		_tag: Schema.Literal("unsupported"),
		capability: GitProviderCapabilityKind,
	}),
]);

export type GitProviderCapability = typeof GitProviderCapability.Type;

/** Describes a provider adapter without leaking its native client or payloads. */
export const GitProviderDescriptor = Schema.Struct({
	capabilities: Schema.Array(GitProviderCapability),
	display_name: BoundedVisibleString,
	provider_id: GitProviderId,
});

export type GitProviderDescriptor = typeof GitProviderDescriptor.Type;

/** Projects optional provider CLI installation without making it a local Git dependency. */
export const GitProviderInstallation = Schema.Union([
	Schema.Struct({ _tag: Schema.Literal("missing") }),
	Schema.Struct({
		_tag: Schema.Literal("available"),
		executable_path: GitProviderExecutablePath,
		version: BoundedVisibleString,
	}),
	Schema.Struct({
		_tag: Schema.Literal("incompatible"),
		executable_path: GitProviderExecutablePath,
		installed_version: BoundedVisibleString,
		reason: BoundedVisibleString,
	}),
	Schema.Struct({
		_tag: Schema.Literal("unavailable"),
		executable_path: Schema.optional(GitProviderExecutablePath),
		reason: BoundedVisibleString,
		version: Schema.optional(BoundedVisibleString),
	}),
]);

export type GitProviderInstallation = typeof GitProviderInstallation.Type;

/** Records an account's authentication state without retaining credentials. */
export const GitProviderAccountAuthentication = Schema.Union([
	Schema.Struct({
		_tag: Schema.Literal("authenticated"),
		account_login: GitProviderAccountLogin,
	}),
	Schema.Struct({
		_tag: Schema.Literal("authentication_required"),
		account_login: Schema.optional(GitProviderAccountLogin),
	}),
	Schema.Struct({
		_tag: Schema.Literal("permission_insufficient"),
		account_login: GitProviderAccountLogin,
	}),
]);

export type GitProviderAccountAuthentication = typeof GitProviderAccountAuthentication.Type;

/** Makes absence of an active account distinct from a selected authenticated account. */
export const GitProviderActiveAccount = Schema.Union([
	Schema.Struct({ _tag: Schema.Literal("none") }),
	Schema.Struct({
		_tag: Schema.Literal("selected"),
		account_login: GitProviderAccountLogin,
	}),
]);

export type GitProviderActiveAccount = typeof GitProviderActiveAccount.Type;

/** Projects authentication for one exact provider host and its known accounts. */
export const GitProviderHostAuthentication = Schema.Struct({
	accounts: Schema.Array(GitProviderAccountAuthentication),
	active_account: GitProviderActiveAccount,
	host: GitProviderHost,
});

export type GitProviderHostAuthentication = typeof GitProviderHostAuthentication.Type;

/** Selects one provider host and account for repository discovery. */
export const GitProviderSelection = Schema.Struct({
	account_login: GitProviderAccountLogin,
	host: GitProviderHost,
	provider_id: GitProviderId,
});

export type GitProviderSelection = typeof GitProviderSelection.Type;

/** Defines the canonical discovery domain instead of accepting provider-specific filters. */
export const GitProviderDiscoveryScope = Schema.Union([
	Schema.Struct({ _tag: Schema.Literal("account") }),
	Schema.Struct({
		_tag: Schema.Literal("organization"),
		organization: GitProviderAccountLogin,
	}),
	Schema.Struct({
		_tag: Schema.Literal("search"),
		query: GitProviderQuery,
	}),
]);

export type GitProviderDiscoveryScope = typeof GitProviderDiscoveryScope.Type;

/** Locates the first repository page or the page after an opaque provider cursor. */
export const GitProviderCursorPosition = Schema.Union([
	Schema.Struct({ _tag: Schema.Literal("first") }),
	Schema.Struct({
		_tag: Schema.Literal("after"),
		cursor: GitProviderCursor,
	}),
]);

export type GitProviderCursorPosition = typeof GitProviderCursorPosition.Type;

/** Preserves only the minimum provider-native repository attribution. */
export const GitProviderRepositoryOrigin = Schema.Struct({
	native_id: BoundedVisibleString,
	provider_id: GitProviderId,
	resource_kind: Schema.Literal("repository"),
});

export type GitProviderRepositoryOrigin = typeof GitProviderRepositoryOrigin.Type;

/** Identifies a repository by its canonical provider host, owner, and name. */
export const GitProviderRepositoryIdentity = Schema.Struct({
	host: GitProviderHost,
	name: BoundedVisibleString,
	owner: GitProviderAccountLogin,
	provider_id: GitProviderId,
});

export type GitProviderRepositoryIdentity = typeof GitProviderRepositoryIdentity.Type;

/** Represents a known default branch or a provider response that did not supply one. */
export const GitProviderDefaultBranch = Schema.Union([
	Schema.Struct({
		_tag: Schema.Literal("known"),
		name: BoundedVisibleString,
	}),
	Schema.Struct({ _tag: Schema.Literal("unavailable") }),
]);

export type GitProviderDefaultBranch = typeof GitProviderDefaultBranch.Type;

/** Projects a discoverable repository without exposing provider response payloads. */
export const GitProviderRepository = Schema.Struct({
	archived: Schema.Boolean,
	clone_url: GitProviderUrl,
	default_branch: GitProviderDefaultBranch,
	identity: GitProviderRepositoryIdentity,
	origin: GitProviderRepositoryOrigin,
	viewer_permission: Schema.Literals(["admin", "maintain", "write", "triage", "read", "unknown"]),
	visibility: Schema.Literals(["private", "public", "internal", "unknown"]),
	web_url: GitProviderWebUrl,
});

export type GitProviderRepository = typeof GitProviderRepository.Type;

/** Distinguishes the final discovery page from a page with an opaque continuation cursor. */
export const GitProviderContinuation = Schema.Union([
	Schema.Struct({ _tag: Schema.Literal("complete") }),
	Schema.Struct({
		_tag: Schema.Literal("more"),
		after: GitProviderCursor,
	}),
]);

export type GitProviderContinuation = typeof GitProviderContinuation.Type;

/** Defines one bounded, canonical repository discovery request. */
export const GitProviderDiscovery = Schema.Struct({
	page_size: Schema.Int.check(Schema.isGreaterThanOrEqualTo(1), Schema.isLessThanOrEqualTo(100)),
	position: GitProviderCursorPosition,
	scope: GitProviderDiscoveryScope,
	selection: GitProviderSelection,
});

export type GitProviderDiscovery = typeof GitProviderDiscovery.Type;

/** Returns a bounded repository projection and its explicit continuation state. */
export const GitProviderPage = Schema.Struct({
	continuation: GitProviderContinuation,
	repositories: Schema.Array(GitProviderRepository),
});

export type GitProviderPage = typeof GitProviderPage.Type;

/** Selects one previously discovered repository for fresh clone preparation. */
export const GitProviderCloneRequest = Schema.Struct({
	repository: GitProviderRepository,
	selection: GitProviderSelection,
});

export type GitProviderCloneRequest = typeof GitProviderCloneRequest.Type;

/** Pins the fresh provider identity that may later cross the clone approval boundary. */
export const GitProviderClonePreparation = Schema.Struct({
	repository: GitProviderRepository,
	selection: GitProviderSelection,
});

export type GitProviderClonePreparation = typeof GitProviderClonePreparation.Type;

/** Supplies the approved destination and pinned provider identity for one clone execution. */
export const GitProviderCloneExecution = Schema.Struct({
	destination: GitProviderCloneDestinationProof,
	preparation: GitProviderClonePreparation,
});

export type GitProviderCloneExecution = typeof GitProviderCloneExecution.Type;

/** Confirms provider execution and the identity observed again after the clone completed. */
export const GitProviderCloneResult = Schema.Struct({
	canonical_root: GitProviderNativePath,
	output_complete: Schema.Boolean,
	repository: GitProviderRepository,
	type: Schema.Literal("cloned"),
});

export type GitProviderCloneResult = typeof GitProviderCloneResult.Type;

/** Binds a hosted read to one selected repository branch and observed local Git head. */
export const GitProviderPullRequestRead = Schema.Struct({
	expected_head: GitObjectId,
	repository: GitProviderRepositoryIdentity,
	selected_branch: GitBranchName,
	selection: GitProviderSelection,
});

export type GitProviderPullRequestRead = typeof GitProviderPullRequestRead.Type;

/** Describes the provider installation and exact hosts known by inspection. */
export const GitProviderInspection = Schema.Struct({
	authentication: Schema.Array(GitProviderHostAuthentication),
	installation: GitProviderInstallation,
});

export type GitProviderInspection = typeof GitProviderInspection.Type;

/** Names the safe operation boundary where a provider adapter failed. */
export const GitProviderErrorOperation = Schema.Literals([
	"clone_repository",
	"discover_repositories",
	"inspect",
	"prepare_clone",
	"read_pull_request",
]);

export type GitProviderErrorOperation = typeof GitProviderErrorOperation.Type;

/** Identifies a safe, canonical provider failure reason without raw client errors. */
export const GitProviderErrorReason = Schema.Literals([
	"account_not_active",
	"auth_required",
	"clone_failed",
	"cli_incompatible",
	"cli_missing",
	"cli_unavailable",
	"git_missing",
	"invalid_cursor",
	"invalid_input",
	"invalid_response",
	"network",
	"not_found",
	"outcome_unknown",
	"permission_denied",
	"rate_limited",
	"remote_rejected",
	"stale_repository",
	"timed_out",
	"unsupported_host",
]);

export type GitProviderErrorReason = typeof GitProviderErrorReason.Type;

/** Reports a provider failure using only bounded canonical metadata. */
export class GitProviderError extends Data.TaggedError("GitProviderError")<{
	readonly host?: GitProviderHost;
	readonly operation: GitProviderErrorOperation;
	readonly provider_id: GitProviderId;
	readonly reason: GitProviderErrorReason;
	readonly retryable: boolean;
}> {}

/** Provides provider-neutral hosted-forge inspection and repository discovery. */
export class GitProvider extends Context.Service<
	GitProvider,
	{
		readonly Descriptor: GitProviderDescriptor;
		readonly DiscoverRepositories: (
			input: GitProviderDiscovery,
		) => Effect.Effect<GitProviderPage, GitProviderError>;
		readonly Inspect: Effect.Effect<GitProviderInspection, GitProviderError>;
		readonly Clone: (
			input: GitProviderCloneExecution,
		) => Effect.Effect<GitProviderCloneResult, GitProviderError>;
		readonly PrepareClone: (
			input: GitProviderCloneRequest,
		) => Effect.Effect<GitProviderClonePreparation, GitProviderError>;
		readonly ReadPullRequest?: (
			input: GitProviderPullRequestRead,
		) => Effect.Effect<typeof HostedGitPullRequestLookup.Type, GitProviderError>;
	}
>()("Artisan/GitProvider") {}
