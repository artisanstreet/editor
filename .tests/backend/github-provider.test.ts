import { Effect, Layer } from "effect";
import { describe, expect, it } from "vitest";

import {
	GitProvider,
	type GitProviderDiscovery,
} from "../../modules/backend/src/git-provider/git-provider";
import {
	GitHubCli,
	GitHubCliError,
	type GitHubCliInspection,
	type GitHubCliRepositoryPage,
} from "../../modules/backend/src/git-provider/github/github-cli";
import { make_github_provider_layer } from "../../modules/backend/src/git-provider/github/github-provider";

const executable_path = "C:\\Program Files\\GitHub CLI\\gh.exe";
const selection = {
	account_login: "alice",
	host: "github.com",
	provider_id: "github",
} as const;

function available_inspection(hosts: Extract<GitHubCliInspection, { type: "available" }>["hosts"]) {
	return {
		executable_path,
		hosts,
		type: "available" as const,
		version: "2.45.1",
	};
}

function repository(name: string, default_branch?: string) {
	return {
		archived: true,
		...(default_branch === undefined ? {} : { default_branch }),
		name,
		name_with_owner: `artisan/${name}`,
		native_id: `node-${name}`,
		owner: "artisan",
		ssh_url: `git@github.com:artisan/${name}.git`,
		updated_at: "2026-07-14T00:00:00Z",
		viewer_permission: "maintain" as const,
		visibility: "internal" as const,
		web_url: `https://github.com/artisan/${name}`,
	};
}

function page(
	viewer_login: string,
	repositories: GitHubCliRepositoryPage["repositories"],
	continuation: GitHubCliRepositoryPage["continuation"] = { type: "complete" },
) {
	return { continuation, repositories, viewer_login };
}

async function make_provider(options: {
	readonly inspection: GitHubCliInspection;
	readonly query?: (
		input: Parameters<(typeof GitHubCli.Service)["QueryRepositories"]>[0],
	) => Effect.Effect<GitHubCliRepositoryPage, GitHubCliError>;
	readonly hosts?: ReadonlyArray<string>;
}) {
	const query = options.query ?? (() => Effect.die("Unexpected repository query"));
	const cli_layer = Layer.succeed(GitHubCli, {
		Inspect: Effect.succeed(options.inspection),
		QueryRepositories: query,
	});
	const provider_layer = make_github_provider_layer(
		options.hosts === undefined ? {} : { hosts: options.hosts },
	).pipe(Layer.provide(cli_layer));

	return Effect.runPromise(Effect.service(GitProvider).pipe(Effect.provide(provider_layer)));
}

function discovery(overrides: Partial<GitProviderDiscovery> = {}) {
	return {
		page_size: 2,
		position: { _tag: "first" as const },
		scope: { _tag: "account" as const },
		selection,
		...overrides,
	};
}

describe("GitHubProvider", () => {
	it("projects CLI installation and multi-host authentication without native credential fields", async () => {
		const available = available_inspection([
			{
				accounts: [
					{
						active: true,
						git_protocol: "ssh",
						host: "github.com",
						login: "alice",
						scopes: ["repo", "read:org"],
						type: "authenticated",
					},
					{
						active: false,
						host: "github.com",
						login: "expired",
						reason: "timeout",
						type: "failed",
					},
				],
				host: "github.com",
			},
			{
				accounts: [
					{
						active: true,
						git_protocol: "https",
						host: "ghe.example",
						login: "carol",
						scopes: ["repo"],
						type: "authenticated",
					},
				],
				host: "ghe.example",
			},
		]);
		const provider = await make_provider({
			hosts: ["configured.example"],
			inspection: available,
		});

		const inspection = await Effect.runPromise(provider.Inspect);

		expect(inspection).toEqual({
			authentication: [
				{
					accounts: [],
					active_account: { _tag: "none" },
					host: "configured.example",
				},
				{
					accounts: [{ _tag: "authenticated", account_login: "carol" }],
					active_account: { _tag: "selected", account_login: "carol" },
					host: "ghe.example",
				},
				{
					accounts: [
						{ _tag: "authenticated", account_login: "alice" },
						{ _tag: "authentication_required", account_login: "expired" },
					],
					active_account: { _tag: "selected", account_login: "alice" },
					host: "github.com",
				},
			],
			installation: {
				_tag: "available",
				executable_path,
				version: "2.45.1",
			},
		});
		expect(JSON.stringify(inspection)).not.toMatch(/scopes|protocol|token|native/i);
	});

	it("maps missing, incompatible, and unavailable CLI states to safe installations", async () => {
		const cases: ReadonlyArray<{
			readonly inspection: GitHubCliInspection;
			readonly installation: unknown;
		}> = [
			{
				inspection: { command: "gh", type: "missing" },
				installation: { _tag: "missing" },
			},
			{
				inspection: {
					executable_path,
					reason: "required_features_missing",
					type: "incompatible",
					version: "2.0.0",
				},
				installation: {
					_tag: "incompatible",
					executable_path,
					installed_version: "2.0.0",
					reason: "Required GitHub CLI JSON authentication features are unavailable",
				},
			},
			{
				inspection: {
					command: "gh --raw-output=secret",
					executable_path,
					reason: "timed_out",
					type: "unavailable",
					version: "2.45.1",
				},
				installation: {
					_tag: "unavailable",
					executable_path,
					reason: "GitHub CLI authentication inspection timed out",
					version: "2.45.1",
				},
			},
		];

		for (const test_case of cases) {
			const provider = await make_provider({ inspection: test_case.inspection });
			const inspection = await Effect.runPromise(provider.Inspect);

			expect(inspection.installation).toEqual(test_case.installation);
			expect(JSON.stringify(inspection)).not.toContain("secret");
		}
	});

	it("rejects invalid, signed-out, and inactive selections before querying repositories", async () => {
		const cases = [
			{
				input: discovery({ selection: { ...selection, provider_id: "gitlab" } }),
				inspection: available_inspection([]),
				reason: "invalid_input",
			},
			{
				input: discovery({ selection: { ...selection, host: "unknown.example" } }),
				inspection: available_inspection([{ accounts: [], host: "github.com" }]),
				reason: "unsupported_host",
			},
			{
				input: discovery(),
				inspection: available_inspection([{ accounts: [], host: "github.com" }]),
				reason: "auth_required",
			},
			{
				input: discovery(),
				inspection: available_inspection([
					{
						accounts: [
							{
								active: true,
								git_protocol: "ssh",
								host: "github.com",
								login: "bob",
								scopes: [],
								type: "authenticated",
							},
						],
						host: "github.com",
					},
				]),
				reason: "account_not_active",
			},
		] as const;

		for (const test_case of cases) {
			let query_count = 0;
			const provider = await make_provider({
				inspection: test_case.inspection,
				query: () => {
					query_count += 1;
					return Effect.die("The provider must reject before querying");
				},
			});

			await expect(
				Effect.runPromise(provider.DiscoverRepositories(test_case.input)),
			).rejects.toMatchObject({
				operation: "discover_repositories",
				reason: test_case.reason,
			});
			expect(query_count).toBe(0);
		}
	});

	it("projects canonical repository identity, safe URLs, and native attribution", async () => {
		const provider = await make_provider({
			inspection: available_inspection([
				{
					accounts: [
						{
							active: true,
							git_protocol: "ssh",
							host: "github.com",
							login: "alice",
							scopes: [],
							type: "authenticated",
						},
					],
					host: "github.com",
				},
			]),
			query: () => Effect.succeed(page("alice", [repository("private-repo")])),
		});

		const result = await Effect.runPromise(provider.DiscoverRepositories(discovery()));

		expect(result.repositories).toEqual([
			{
				archived: true,
				clone_url: "git@github.com:artisan/private-repo.git",
				default_branch: { _tag: "unavailable" },
				identity: {
					host: "github.com",
					name: "private-repo",
					owner: "artisan",
					provider_id: "github",
				},
				origin: {
					native_id: "node-private-repo",
					provider_id: "github",
					resource_kind: "repository",
				},
				viewer_permission: "maintain",
				visibility: "internal",
				web_url: "https://github.com/artisan/private-repo",
			},
		]);
	});

	it("rejects repository pages that exceed the requested canonical bound", async () => {
		const provider = await make_provider({
			inspection: available_inspection([
				{
					accounts: [
						{
							active: true,
							git_protocol: "ssh",
							host: "github.com",
							login: "alice",
							scopes: [],
							type: "authenticated",
						},
					],
					host: "github.com",
				},
			]),
			query: () =>
				Effect.succeed(
					page("alice", [repository("one"), repository("two"), repository("three")]),
				),
		});

		await expect(
			Effect.runPromise(provider.DiscoverRepositories(discovery())),
		).rejects.toMatchObject({
			operation: "discover_repositories",
			reason: "invalid_response",
		});
	});

	it("rejects repository URLs that escape the selected provider host", async () => {
		const cases = [
			{ ...repository("clone-escape"), ssh_url: "git@evil.example:artisan/repo.git" },
			{ ...repository("web-escape"), web_url: "https://evil.example/artisan/repo" },
		];

		for (const escaped_repository of cases) {
			const provider = await make_provider({
				inspection: available_inspection([
					{
						accounts: [
							{
								active: true,
								git_protocol: "ssh",
								host: "github.com",
								login: "alice",
								scopes: [],
								type: "authenticated",
							},
						],
						host: "github.com",
					},
				]),
				query: () => Effect.succeed(page("alice", [escaped_repository])),
			});

			await expect(
				Effect.runPromise(provider.DiscoverRepositories(discovery())),
			).rejects.toMatchObject({
				operation: "discover_repositories",
				reason: "invalid_response",
			});
		}
	});

	it("binds opaque search continuations to the full selection and request", async () => {
		const query_inputs: Array<Parameters<(typeof GitHubCli.Service)["QueryRepositories"]>[0]> =
			[];
		const long_query = "repository topic:effect archived:false ".repeat(6).trim();
		const provider = await make_provider({
			inspection: available_inspection([
				{
					accounts: [
						{
							active: true,
							git_protocol: "ssh",
							host: "github.com",
							login: "alice",
							scopes: [],
							type: "authenticated",
						},
					],
					host: "github.com",
				},
			]),
			query: (input) => {
				query_inputs.push(input);

				return Effect.succeed(
					input.native_cursor === undefined
						? page("alice", [repository("first", "main")], {
								cursor: "next-native",
								type: "more",
							})
						: page("alice", [repository("second", "main")]),
				);
			},
		});
		const first_input = discovery({ scope: { _tag: "search", query: long_query } });
		const first_page = await Effect.runPromise(provider.DiscoverRepositories(first_input));

		expect(first_page.continuation._tag).toBe("more");

		if (first_page.continuation._tag !== "more") {
			throw new Error("Expected a continuation cursor");
		}
		expect(first_page.continuation.after).not.toBe("next-native");

		const continuation_input = {
			...first_input,
			position: { _tag: "after" as const, cursor: first_page.continuation.after },
		};
		const continuation_page = await Effect.runPromise(
			provider.DiscoverRepositories(continuation_input),
		);
		const rejected_inputs = [
			{ ...continuation_input, page_size: 3 },
			{ ...continuation_input, scope: { _tag: "account" as const } },
			{ ...continuation_input, selection: { ...selection, account_login: "bob" } },
			{ ...continuation_input, selection: { ...selection, host: "ghe.example" } },
			{ ...continuation_input, selection: { ...selection, provider_id: "gitlab" } },
		] as const;

		expect(continuation_page.repositories).toMatchObject([{ identity: { name: "second" } }]);
		for (const input of rejected_inputs) {
			await expect(
				Effect.runPromise(provider.DiscoverRepositories(input)),
			).rejects.toMatchObject({
				operation: "discover_repositories",
			});
		}

		expect(query_inputs).toHaveLength(2);
	});

	it("rejects a query served by a different account and maps CLI failures without raw output", async () => {
		const cases = [
			{
				query: () => Effect.succeed(page("bob", [])),
				reason: "account_not_active",
			},
			{
				query: () =>
					Effect.fail(
						Object.assign(
							new GitHubCliError({
								operation: "query_repositories",
								reason: "rate_limited",
								retryable: true,
							}),
							{ raw_output: "raw-cli-output-secret" },
						),
					),
				reason: "rate_limited",
			},
			{
				query: () =>
					Effect.fail(
						new GitHubCliError({
							operation: "query_repositories",
							reason: "authentication_required",
							retryable: false,
						}),
					),
				reason: "auth_required",
			},
		] as const;

		for (const test_case of cases) {
			const provider = await make_provider({
				inspection: available_inspection([
					{
						accounts: [
							{
								active: true,
								git_protocol: "ssh",
								host: "github.com",
								login: "alice",
								scopes: [],
								type: "authenticated",
							},
						],
						host: "github.com",
					},
				]),
				query: test_case.query,
			});
			const error = await Effect.runPromise(
				provider.DiscoverRepositories(discovery()).pipe(Effect.flip),
			);

			expect(error).toMatchObject({
				host: "github.com",
				operation: "discover_repositories",
				provider_id: "github",
				reason: test_case.reason,
			});
			expect(JSON.stringify(error)).not.toContain("raw-cli-output-secret");
		}
	});
});
