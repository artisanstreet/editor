import { Effect, FileSystem, Layer, Option, Path } from "effect";

import {
	GitTransportAuthentication,
	GitTransportAuthenticationError,
	type GitTransportAuthorization,
	type GitTransportAuthenticationRequest,
} from "../git-transport-authentication";
import { normalize_git_provider_host } from "../git-provider";
import { make_github_git_transport_environment } from "./github-cli";
import { GitHubCliExecutable, GitHubCliGitExecutable } from "./github-cli-executable";

const github_provider_id = "github";

/** Configures GitHub-selected-account authentication for local child Git invocations. */
export interface GitHubGitTransportAuthenticationOptions {
	readonly cwd: string;
}

function authentication_error(reason: GitTransportAuthenticationError["reason"]) {
	return new GitTransportAuthenticationError({ reason });
}

function valid_account_login(value: string) {
	const bytes = new TextEncoder().encode(value);

	return value.trim().length > 0 && bytes.byteLength <= 128 && !/[\p{Cc}\p{Cf}\s]/u.test(value);
}

function valid_remote_endpoint(value: string, host: string) {
	const bytes = new TextEncoder().encode(value);

	try {
		const endpoint = new URL(value);

		return (
			bytes.byteLength <= 4_096 &&
			endpoint.protocol === "https:" &&
			endpoint.host === host &&
			endpoint.username.length === 0 &&
			endpoint.password.length === 0 &&
			endpoint.pathname !== "/" &&
			endpoint.search.length === 0 &&
			endpoint.hash.length === 0
		);
	} catch {
		return false;
	}
}

function ValidateGitHubAuthenticationRequest(input: GitTransportAuthenticationRequest) {
	const host = normalize_git_provider_host(input.host);

	if (input.provider_id !== github_provider_id) {
		return Effect.fail(authentication_error("unsupported_provider"));
	}

	if (
		host === undefined ||
		host !== input.host ||
		!valid_account_login(input.account_login) ||
		!valid_remote_endpoint(input.remote_endpoint, host)
	) {
		return Effect.fail(authentication_error("invalid_request"));
	}

	return Effect.succeed({
		account_login: input.account_login,
		host,
		remote_endpoint: input.remote_endpoint,
	});
}

/** Builds the GitHub implementation of backend-private child Git authorization. */
export function make_github_git_transport_authentication_layer(
	options: GitHubGitTransportAuthenticationOptions,
) {
	return Layer.effect(
		GitTransportAuthentication,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			const path_service = yield* Path.Path;
			const executable = yield* GitHubCliExecutable;
			const git_executable = yield* GitHubCliGitExecutable;
			const WithAuthorization: (typeof GitTransportAuthentication.Service)["WithAuthorization"] =
				<A, E, R>(
					input: GitTransportAuthenticationRequest,
					use: (authorization: GitTransportAuthorization) => Effect.Effect<A, E, R>,
				) =>
					Effect.scoped(
						Effect.gen(function* () {
							const request = yield* ValidateGitHubAuthenticationRequest(input);
							const gh_location = yield* executable.Locate;
							const git_location = yield* git_executable.Locate;

							if (Option.isNone(gh_location) || Option.isNone(git_location)) {
								return yield* Effect.fail(
									authentication_error("dependency_missing"),
								);
							}

							const private_home = yield* file_system
								.makeTempDirectoryScoped({
									prefix: "artisan-github-git-transport-",
								})
								.pipe(
									Effect.mapError(() =>
										authentication_error("authentication_required"),
									),
								);
							const environment = make_github_git_transport_environment(request, {
								cwd: options.cwd,
								gh_executable_path: gh_location.value.path,
								git_executable_path: git_location.value.path,
								path_service,
								private_home,
							});

							if (environment === undefined) {
								return yield* Effect.fail(
									authentication_error("authentication_required"),
								);
							}

							return yield* use({
								environment,
								git_executable_path: git_location.value.path,
								remote_endpoint: request.remote_endpoint,
								transport_protocol: "https",
							});
						}),
					);

			return { WithAuthorization };
		}),
	);
}
