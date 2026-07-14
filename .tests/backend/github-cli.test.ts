import { Buffer } from "node:buffer";

import { Effect, Layer, Option } from "effect";
import { describe, expect, it } from "vitest";

import {
	GitHubCli,
	GitHubCliError,
	make_github_cli_layer,
} from "../../modules/backend/src/git-provider/github/github-cli";
import { GitHubCliExecutable } from "../../modules/backend/src/git-provider/github/github-cli-executable";
import {
	ProcessRunner,
	ProcessRunnerError,
	type ProcessRunnerInput,
	type ProcessRunnerResult,
} from "../../modules/backend/src/git/process-runner";

const executable_path = "C:\\Program Files\\GitHub CLI\\gh.exe";

function process_result(
	stdout: string,
	options: Partial<ProcessRunnerResult> = {},
): ProcessRunnerResult {
	const stdout_bytes = Buffer.from(stdout);
	const stderr = Buffer.from(options.stderr ?? "");

	return {
		exit_code: options.exit_code ?? 0,
		stderr,
		stderr_bytes: options.stderr_bytes ?? stderr.byteLength,
		stderr_truncated: options.stderr_truncated ?? false,
		stdout: stdout_bytes,
		stdout_bytes: options.stdout_bytes ?? stdout_bytes.byteLength,
		stdout_truncated: options.stdout_truncated ?? false,
	};
}

function executable_layer() {
	return Layer.succeed(GitHubCliExecutable, {
		Locate: Effect.succeed(Option.some({ path: executable_path })),
	});
}

async function make_cli(
	run: (input: ProcessRunnerInput) => Effect.Effect<ProcessRunnerResult, ProcessRunnerError>,
) {
	return Effect.runPromise(
		Effect.service(GitHubCli).pipe(
			Effect.provide(
				make_github_cli_layer({ cwd: "C:\\artisan\\project" }).pipe(
					Layer.provide(Layer.succeed(ProcessRunner, { Run: run })),
					Layer.provide(executable_layer()),
				),
			),
		),
	);
}

function auth_status() {
	return JSON.stringify({
		hosts: {
			"github.com": [
				{
					active: true,
					error: undefined,
					gitProtocol: "ssh",
					host: "github.com",
					login: "alice",
					scopes: "repo, read:org, repo",
					state: "success",
					tokenSource: "oauth_token",
				},
				{
					active: false,
					error: "expired",
					gitProtocol: "https",
					host: "github.com",
					login: "bob",
					scopes: undefined,
					state: "timeout",
					tokenSource: "oauth_token",
				},
			],
			"ghe.example": [
				{
					active: true,
					error: undefined,
					gitProtocol: "https",
					host: "ghe.example",
					login: "carol",
					scopes: "user",
					state: "success",
					tokenSource: "oauth_token",
				},
			],
		},
	});
}

describe("GitHubCli", () => {
	it("returns missing without spawning and projects safe multi-host authentication", async () => {
		let spawn_count = 0;
		const cli = await Effect.runPromise(
			Effect.service(GitHubCli).pipe(
				Effect.provide(
					make_github_cli_layer({ cwd: "C:\\artisan\\project" }).pipe(
						Layer.provide(
							Layer.succeed(GitHubCliExecutable, {
								Locate: Effect.succeed(Option.none()),
							}),
						),
						Layer.provide(
							Layer.succeed(ProcessRunner, {
								Run: () => {
									spawn_count += 1;
									return Effect.succeed(process_result(""));
								},
							}),
						),
					),
				),
			),
		);

		expect(await Effect.runPromise(cli.Inspect)).toEqual({ command: "gh", type: "missing" });
		expect(spawn_count).toBe(0);

		const calls: Array<ProcessRunnerInput> = [];
		const available = await make_cli((input) => {
			calls.push(input);
			return Effect.succeed(
				input.args[0] === "version"
					? process_result("gh version 2.45.1 (2024-01-01)\n")
					: process_result(auth_status()),
			);
		});
		await expect(Effect.runPromise(available.Inspect)).resolves.toEqual({
			executable_path,
			hosts: [
				{
					accounts: [
						{
							active: true,
							git_protocol: "ssh",
							host: "github.com",
							login: "alice",
							scopes: ["read:org", "repo"],
							type: "authenticated",
						},
						{
							active: false,
							host: "github.com",
							login: "bob",
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
							scopes: ["user"],
							type: "authenticated",
						},
					],
					host: "ghe.example",
				},
			],
			version: "2.45.1",
			type: "available",
		});
		expect(calls).toHaveLength(2);
		expect(calls.every((input) => input.command === executable_path)).toBe(true);
		expect(calls.every((input) => input.cwd === "C:\\artisan\\project")).toBe(true);
		expect(calls.map((input) => input.environment)).toEqual([
			{
				GH_NO_UPDATE_NOTIFIER: "1",
				GH_PAGER: "cat",
				GH_PROMPT_DISABLED: "1",
				NO_COLOR: "1",
			},
			{
				GH_NO_UPDATE_NOTIFIER: "1",
				GH_PAGER: "cat",
				GH_PROMPT_DISABLED: "1",
				NO_COLOR: "1",
			},
		]);
		expect(calls[1]?.args).toEqual(["auth", "status", "--json", "hosts"]);
		expect(calls.some((input) => input.args.includes("--show-token"))).toBe(false);
	});

	it("classifies incompatible, malformed, truncated, and failed auth probes as unavailable", async () => {
		const cases = [
			{
				name: "unknown json flag",
				result: (input: ProcessRunnerInput) =>
					input.args[0] === "version"
						? Effect.succeed(process_result("gh version 2.45.1\n"))
						: Effect.succeed(process_result("unknown flag: --json", { exit_code: 1 })),
				expected: {
					executable_path,
					reason: "required_features_missing",
					type: "incompatible",
					version: "2.45.1",
				},
			},
			{
				name: "malformed auth output",
				result: (input: ProcessRunnerInput) =>
					input.args[0] === "version"
						? Effect.succeed(process_result("gh version 2.45.1\n"))
						: Effect.succeed(process_result("{")),
				expected: {
					command: "gh",
					executable_path,
					reason: "invalid_output",
					type: "unavailable",
					version: "2.45.1",
				},
			},
			{
				name: "truncated auth output",
				result: (input: ProcessRunnerInput) =>
					input.args[0] === "version"
						? Effect.succeed(process_result("gh version 2.45.1\n"))
						: Effect.succeed(process_result("{}", { stdout_truncated: true })),
				expected: {
					command: "gh",
					executable_path,
					reason: "invalid_output",
					type: "unavailable",
					version: "2.45.1",
				},
			},
			{
				name: "auth process failure",
				result: (input: ProcessRunnerInput) =>
					input.args[0] === "version"
						? Effect.succeed(process_result("gh version 2.45.1\n"))
						: Effect.fail(
								new ProcessRunnerError({
									cause: "failed",
									command: input.command,
									operation: "spawn",
								}),
							),
				expected: {
					command: "gh",
					executable_path,
					reason: "process_failed",
					type: "unavailable",
					version: "2.45.1",
				},
			},
		];

		for (const test_case of cases) {
			const cli = await make_cli(test_case.result);

			await expect(Effect.runPromise(cli.Inspect), test_case.name).resolves.toEqual(
				test_case.expected,
			);
		}
	});

	it("queries account, organization, and search roots with continuation and null projections", async () => {
		const outputs = [
			{
				viewer: {
					login: "alice",
					repositories: {
						nodes: [],
						pageInfo: { hasNextPage: true, endCursor: "account-next" },
					},
				},
			},
			{
				viewer: { login: "alice" },
				organization: {
					repositories: {
						nodes: [repository("org/repo", null, null)],
						pageInfo: { hasNextPage: false, endCursor: null },
					},
				},
			},
			{
				viewer: { login: "alice" },
				search: {
					nodes: [repository("alice/repo", "main", "WRITE")],
					pageInfo: { hasNextPage: false, endCursor: null },
				},
			},
		];
		const calls: Array<ProcessRunnerInput> = [];
		const cli = await make_cli((input) => {
			calls.push(input);
			return Effect.succeed(
				process_result(JSON.stringify({ data: outputs[calls.length - 1] })),
			);
		});

		await expect(
			Effect.runPromise(
				cli.QueryRepositories({
					host: "github.com",
					page_size: 10,
					scope: { _tag: "account" },
				}),
			),
		).resolves.toMatchObject({
			continuation: { cursor: "account-next", type: "more" },
		});
		const organization_page = await Effect.runPromise(
			cli.QueryRepositories({
				host: "ghe.example",
				native_cursor: "cursor",
				page_size: 10,
				scope: { _tag: "organization", organization: "artisan" },
			}),
		);
		expect(organization_page.repositories[0]).toMatchObject({ viewer_permission: "unknown" });
		expect(organization_page.repositories[0]?.default_branch).toBeUndefined();
		await expect(
			Effect.runPromise(
				cli.QueryRepositories({
					host: "github.com",
					page_size: 10,
					scope: { _tag: "search", query: "is:open" },
				}),
			),
		).resolves.toMatchObject({
			repositories: [{ default_branch: "main", viewer_permission: "write" }],
		});

		expect(calls.every((input) => input.command === executable_path)).toBe(true);
		expect(calls.map((input) => input.args.slice(0, 6))).toEqual([
			["api", "graphql", "--hostname", "github.com", "--method", "POST"],
			["api", "graphql", "--hostname", "ghe.example", "--method", "POST"],
			["api", "graphql", "--hostname", "github.com", "--method", "POST"],
		]);
		expect(calls[1]?.args).toContain("after=cursor");
		expect(calls[1]?.args).toContain("organization=artisan");
		expect(calls[2]?.args).toContain("search=is:open");
		expect(calls.every((input) => !input.args.includes("auth"))).toBe(true);
	});

	it("classifies API rate limits, permission denial, and malformed or truncated output safely", async () => {
		const cases = [
			{
				output: JSON.stringify({ errors: [{ message: "API rate limit exceeded" }] }),
				reason: "rate_limited",
				retryable: true,
			},
			{
				output: JSON.stringify({
					errors: [{ message: "Resource not accessible by integration" }],
				}),
				reason: "permission_insufficient",
				retryable: false,
			},
			{
				output: "HTTP 503: Service Unavailable",
				reason: "remote_rejected",
				retryable: true,
			},
			{ output: "not json", reason: "remote_rejected", retryable: false },
			{ output: "{}", truncated: true, reason: "invalid_response", retryable: false },
		] as const;

		for (const test_case of cases) {
			const cli = await make_cli(() =>
				Effect.succeed(
					process_result(test_case.output, {
						exit_code: 1,
						stdout_truncated: "truncated" in test_case ? test_case.truncated : false,
					}),
				),
			);
			const error = await Effect.runPromise(
				cli
					.QueryRepositories({
						host: "github.com",
						page_size: 10,
						scope: { _tag: "account" },
					})
					.pipe(Effect.flip),
			);

			expect(error).toBeInstanceOf(GitHubCliError);
			expect(error).toMatchObject({
				operation: "query_repositories",
				reason: test_case.reason,
				retryable: test_case.retryable,
			});
			expect(JSON.stringify(error)).not.toContain(test_case.output);
		}
	});
});

function repository(
	name_with_owner: string,
	default_branch: string | null,
	viewer_permission: "WRITE" | null,
) {
	const [owner, name] = name_with_owner.split("/");

	return {
		defaultBranchRef: default_branch === null ? null : { name: default_branch },
		id: `id-${name}`,
		isArchived: false,
		name,
		nameWithOwner: name_with_owner,
		owner: { login: owner },
		sshUrl: `git@github.com:${name_with_owner}.git`,
		updatedAt: "2026-07-14T00:00:00Z",
		url: `https://github.com/${name_with_owner}`,
		viewerPermission: viewer_permission,
		visibility: "PRIVATE",
	};
}
