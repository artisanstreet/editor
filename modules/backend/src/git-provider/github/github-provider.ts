import { Buffer } from "node:buffer";

import { Cause, Data, Effect, Layer, Schema } from "effect";

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
} from "../git-provider";
import {
	GitHubCli,
	GitHubCliError,
	github_https_clone_url,
	type GitHubCliAccount,
	type GitHubCliInspection,
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

			return {
				Descriptor: {
					capabilities: [
						{ _tag: "available", capability: "inspect_authentication" },
						{ _tag: "available", capability: "discover_repositories" },
						{ _tag: "available", capability: "clone_repository" },
						{
							_tag: "unavailable",
							capability: "read_reviews",
							reason: "Review projection is a later durable milestone",
						},
						{
							_tag: "unavailable",
							capability: "read_ci",
							reason: "CI projection is a later durable milestone",
						},
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
			};
		}),
	);
}
