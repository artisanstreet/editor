import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, Option } from "effect";
import { describe, expect, it } from "vitest";

import {
	GitTransportAuthentication,
	GitTransportAuthenticationError,
	type GitTransportAuthenticationRequest,
	type GitTransportAuthorization,
} from "../../modules/backend/src/git-provider/git-transport-authentication";
import { make_github_git_transport_authentication_layer } from "../../modules/backend/src/git-provider/github/github-transport-authentication";
import {
	GitHubCliExecutable,
	GitHubCliGitExecutable,
} from "../../modules/backend/src/git-provider/github/github-cli-executable";

const platform_layer = Layer.merge(NodeFileSystem.layer, NodePath.layer);

function executable_layer(options: { readonly gh?: string; readonly git?: string } = {}) {
	const gh = options.gh ?? "C:\\Program Files\\GitHub CLI\\gh.exe";
	const git = options.git ?? "C:\\Program Files\\Git\\cmd\\git.exe";

	return Layer.mergeAll(
		Layer.succeed(GitHubCliExecutable, {
			Locate: Effect.succeed(gh === "" ? Option.none() : Option.some({ path: gh })),
		}),
		Layer.succeed(GitHubCliGitExecutable, {
			Locate: Effect.succeed(git === "" ? Option.none() : Option.some({ path: git })),
		}),
	);
}

function make_layer(options: { readonly gh?: string; readonly git?: string } = {}) {
	return make_github_git_transport_authentication_layer({ cwd: "C:\\artisan\\project" }).pipe(
		Layer.provide(executable_layer(options)),
		Layer.provide(platform_layer),
	);
}

function with_authorization<A, E, R>(
	input: GitTransportAuthenticationRequest,
	use: (authorization: GitTransportAuthorization) => Effect.Effect<A, E, R>,
) {
	return Effect.gen(function* () {
		const authentication = yield* GitTransportAuthentication;

		return yield* authentication.WithAuthorization(input, use);
	});
}

describe("GitHubGitTransportAuthentication", () => {
	it("binds one github.com account while isolating ambient credential sources", async () => {
		const authorization = await Effect.runPromise(
			with_authorization(
				{
					account_login: "alice",
					host: "github.com",
					provider_id: "github",
					remote_endpoint: "https://github.com/artisan/editor.git",
				},
				Effect.succeed,
			).pipe(Effect.provide(make_layer())),
		);

		expect(authorization.environment).toMatchObject({
			ARTISAN_GH_ACCOUNT: "alice",
			ARTISAN_GH_HOST: "github.com",
			GCM_INTERACTIVE: "Never",
			GH_CONFIG_DIR: undefined,
			GIT_CONFIG_COUNT: "4",
			GIT_CONFIG_KEY_1: "credential.helper",
			GIT_CONFIG_KEY_2: "credential.helper",
			GIT_CONFIG_VALUE_1: "",
			GIT_TERMINAL_PROMPT: "0",
		});
		expect(authorization.environment.GIT_CONFIG_VALUE_2).toContain(
			'"$ARTISAN_GH_EXECUTABLE" auth token',
		);
		expect(authorization.environment).not.toHaveProperty("GH_TOKEN");
		expect(authorization.environment).not.toHaveProperty("GH_ENTERPRISE_TOKEN");
		expect(authorization.git_executable_path).toBe("C:\\Program Files\\Git\\cmd\\git.exe");
		expect(authorization.remote_endpoint).toBe("https://github.com/artisan/editor.git");
		expect(authorization.transport_protocol).toBe("https");
	});

	it("binds GitHub Enterprise to its exact selected host", async () => {
		const authorization = await Effect.runPromise(
			with_authorization(
				{
					account_login: "enterprise-alice",
					host: "github.artisan.test:8443",
					provider_id: "github",
					remote_endpoint: "https://github.artisan.test:8443/artisan/editor.git",
				},
				Effect.succeed,
			).pipe(Effect.provide(make_layer())),
		);

		expect(authorization.environment.ARTISAN_GH_ACCOUNT).toBe("enterprise-alice");
		expect(authorization.environment.ARTISAN_GH_HOST).toBe("github.artisan.test:8443");
		expect(authorization.git_executable_path).toBe("C:\\Program Files\\Git\\cmd\\git.exe");
		expect(authorization.remote_endpoint).toBe(
			"https://github.artisan.test:8443/artisan/editor.git",
		);
		expect(authorization.transport_protocol).toBe("https");
	});

	it("rejects an unsupported provider before creating authorization", async () => {
		const error = await Effect.runPromise(
			with_authorization(
				{
					account_login: "alice",
					host: "github.com",
					provider_id: "gitlab",
					remote_endpoint: "https://github.com/artisan/editor.git",
				},
				Effect.succeed,
			).pipe(Effect.provide(make_layer()), Effect.flip),
		);

		expect(error).toEqual(
			new GitTransportAuthenticationError({ reason: "unsupported_provider" }),
		);
	});

	it("rejects a noncanonical host or invalid account before creating authorization", async () => {
		const cases = [
			{
				account_login: "alice",
				host: "GitHub.com",
				provider_id: "github",
				remote_endpoint: "https://github.com/artisan/editor.git",
			},
			{
				account_login: "alice token",
				host: "github.com",
				provider_id: "github",
				remote_endpoint: "https://github.com/artisan/editor.git",
			},
			{
				account_login: "alice",
				host: "github.com",
				provider_id: "github",
				remote_endpoint: "https://example.com/artisan/editor.git",
			},
		];

		for (const input of cases) {
			const error = await Effect.runPromise(
				with_authorization(input, Effect.succeed).pipe(
					Effect.provide(make_layer()),
					Effect.flip,
				),
			);

			expect(error).toEqual(
				new GitTransportAuthenticationError({ reason: "invalid_request" }),
			);
		}
	});

	it("fails closed when either GitHub CLI or Git is unavailable", async () => {
		for (const options of [{ gh: "" }, { git: "" }]) {
			const error = await Effect.runPromise(
				with_authorization(
					{
						account_login: "alice",
						host: "github.com",
						provider_id: "github",
						remote_endpoint: "https://github.com/artisan/editor.git",
					},
					Effect.succeed,
				).pipe(Effect.provide(make_layer(options)), Effect.flip),
			);

			expect(error).toEqual(
				new GitTransportAuthenticationError({ reason: "dependency_missing" }),
			);
		}
	});

	it("keeps the scoped private home unavailable after authorization returns", async () => {
		const private_home = await Effect.runPromise(
			with_authorization(
				{
					account_login: "alice",
					host: "github.com",
					provider_id: "github",
					remote_endpoint: "https://github.com/artisan/editor.git",
				},
				(authorization) => Effect.succeed(authorization.environment.HOME),
			).pipe(Effect.provide(make_layer())),
		);
		const file_system = await Effect.runPromise(
			Effect.service(FileSystem.FileSystem).pipe(Effect.provide(platform_layer)),
		);

		expect(private_home).toBeDefined();
		expect(await Effect.runPromise(file_system.exists(private_home!))).toBe(false);
	});
});
