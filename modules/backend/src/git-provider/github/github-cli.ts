import { Buffer } from "node:buffer";

import { Context, Data, Effect, Layer, Option, Schema } from "effect";

import { ProcessRunner, type ProcessRunnerResult } from "../../git/process-runner";
import type { GitProviderDiscoveryScope } from "../git-provider";
import { GitHubCliExecutable } from "./github-cli-executable";

const VersionPattern = /^gh version (\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)(?:\s|$)/mu;

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

/** Classifies GitHub CLI failures without retaining provider output in the error. */
export class GitHubCliError extends Data.TaggedError("GitHubCliError")<{
	readonly operation: "query_repositories";
	readonly reason:
		| "authentication_required"
		| "dependency_incompatible"
		| "dependency_missing"
		| "invalid_response"
		| "network_unavailable"
		| "permission_insufficient"
		| "process_failed"
		| "rate_limited"
		| "remote_not_found"
		| "remote_rejected";
	readonly retryable: boolean;
}> {}

/** Rejects invalid process limits before a GitHub CLI service is published. */
export class GitHubCliConfigurationError extends Data.TaggedError("GitHubCliConfigurationError")<{
	readonly field: "probe_timeout_ms" | "request_timeout_ms";
}> {}

/** Owns every GitHub CLI subprocess and provider-native decoder. */
export class GitHubCli extends Context.Service<
	GitHubCli,
	{
		readonly Inspect: Effect.Effect<GitHubCliInspection>;
		readonly QueryRepositories: (
			input: GitHubCliRepositoryQuery,
		) => Effect.Effect<GitHubCliRepositoryPage, GitHubCliError>;
	}
>()("Artisan/GitHubCli") {}

/** Configures bounded, non-interactive GitHub CLI subprocesses. */
export interface GitHubCliOptions {
	readonly command?: string;
	readonly cwd: string;
	readonly probe_timeout_ms?: number;
	readonly request_timeout_ms?: number;
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

function api_error(reason: GitHubCliError["reason"], retryable: boolean) {
	return new GitHubCliError({
		operation: "query_repositories",
		reason,
		retryable,
	});
}

function classify_api_failure(
	result: ProcessRunnerResult,
	envelope: Option.Option<typeof GitHubGraphqlEnvelope.Type>,
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
		return api_error("rate_limited", true);
	}

	if (
		output.includes("resource not accessible") ||
		output.includes("forbidden") ||
		output.includes("http 403")
	) {
		return api_error("permission_insufficient", false);
	}

	if (
		output.includes("authentication") ||
		output.includes("not logged") ||
		output.includes("bad credentials") ||
		output.includes("http 401")
	) {
		return api_error("authentication_required", false);
	}

	if (
		output.includes("could not resolve") ||
		output.includes("connection refused") ||
		output.includes("failed to connect") ||
		output.includes("network is unreachable")
	) {
		return api_error("network_unavailable", true);
	}

	if (output.includes("not found") || output.includes("not_found")) {
		return api_error("remote_not_found", false);
	}

	const remote_temporarily_unavailable =
		/\bhttp(?: status)?[\s:]+5\d{2}\b/u.test(output) ||
		output.includes("internal server error") ||
		output.includes("temporarily unavailable") ||
		output.includes("service unavailable") ||
		output.includes("bad gateway") ||
		output.includes("gateway timeout");

	return api_error("remote_rejected", remote_temporarily_unavailable);
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

function valid_timeout(value: number) {
	return Number.isSafeInteger(value) && value > 0 && value <= 5 * 60_000;
}

/** Builds the dedicated GitHub CLI capability over the shared bounded process runner. */
export function make_github_cli_layer(options: GitHubCliOptions) {
	const command = options.command ?? "gh";
	const probe_timeout_ms = options.probe_timeout_ms ?? 10_000;
	const request_timeout_ms = options.request_timeout_ms ?? 30_000;

	return Layer.effect(
		GitHubCli,
		Effect.gen(function* () {
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
			const runner = yield* ProcessRunner;
			const environment = {
				GH_NO_UPDATE_NOTIFIER: "1",
				GH_PAGER: "cat",
				GH_PROMPT_DISABLED: "1",
				NO_COLOR: "1",
			};
			const Run = (
				executable_path: string,
				args: ReadonlyArray<string>,
				max_stdout_bytes: number,
				timeout_ms: number,
			) =>
				runner
					.Run({
						args,
						command: executable_path,
						cwd: options.cwd,
						environment,
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

			return { Inspect, QueryRepositories };
		}),
	);
}
