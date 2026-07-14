import { Buffer } from "node:buffer";
import { dirname } from "node:path";

import { NodeCrypto, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Cause, Deferred, Effect, Exit, Fiber, FileSystem, Layer, Option, Path } from "effect";
import { describe, expect, it } from "vitest";

import {
	GitHubCli,
	GitHubCliError,
	make_github_cli_layer,
	type GitHubCliOptions,
} from "../../modules/backend/src/git-provider/github/github-cli";
import {
	GitHubCliExecutable,
	GitHubCliGitExecutable,
} from "../../modules/backend/src/git-provider/github/github-cli-executable";
import {
	ProcessRunner,
	ProcessRunnerError,
	type ProcessRunnerInput,
	type ProcessRunnerResult,
} from "../../modules/backend/src/git/process-runner";
import { NodeProcessRunnerLive } from "../../modules/backend/src/git/node-process-runner";
import { ReadFileIdentity } from "../../modules/backend/src/filesystem/file-identity";

const executable_path = "C:\\Program Files\\GitHub CLI\\gh.exe";
const git_executable_path =
	process.platform === "win32" ? "C:\\Program Files\\Git\\cmd\\git.exe" : "/usr/bin/git";

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

const platform_layer = Layer.mergeAll(NodeCrypto.layer, NodeFileSystem.layer, NodePath.layer);

function executable_layer(
	gh_path = executable_path,
	git_path: string | null = git_executable_path,
) {
	return Layer.mergeAll(
		Layer.succeed(GitHubCliExecutable, {
			Locate: Effect.succeed(Option.some({ path: gh_path })),
		}),
		Layer.succeed(GitHubCliGitExecutable, {
			Locate: Effect.succeed(
				git_path === null ? Option.none() : Option.some({ path: git_path }),
			),
		}),
	);
}

async function make_cli(
	run: (input: ProcessRunnerInput) => Effect.Effect<ProcessRunnerResult, ProcessRunnerError>,
	options: Partial<GitHubCliOptions> = {},
	locations: { readonly gh_path?: string; readonly git_path?: string | null } = {},
) {
	const cwd = options.cwd ?? "C:\\artisan\\project";
	const projects_root = options.projects_root ?? cwd;

	return Effect.runPromise(
		Effect.service(GitHubCli).pipe(
			Effect.provide(
				make_github_cli_layer({ ...options, cwd, projects_root }).pipe(
					Layer.provide(Layer.succeed(ProcessRunner, { Run: run })),
					Layer.provide(executable_layer(locations.gh_path, locations.git_path)),
					Layer.provide(platform_layer),
				),
			),
		),
	);
}

async function test_platform() {
	return Effect.runPromise(
		Effect.all({
			file_system: Effect.service(FileSystem.FileSystem),
			path_service: Effect.service(Path.Path),
		}).pipe(Effect.provide(platform_layer)),
	);
}

async function with_temporary_directory<A>(use: (root: string) => Promise<A>) {
	const { file_system } = await test_platform();
	const root = await Effect.runPromise(
		file_system.makeTempDirectory({ prefix: "artisan-github-cli-test-" }),
	);

	try {
		return await use(root);
	} finally {
		await Effect.runPromise(file_system.remove(root, { recursive: true }));
	}
}

async function with_process_environment<A>(
	environment: Readonly<Record<string, string>>,
	use: () => Promise<A>,
) {
	const previous = Object.fromEntries(
		Object.keys(environment).map((key) => [key, process.env[key]]),
	);

	for (const [key, value] of Object.entries(environment)) {
		process.env[key] = value;
	}

	try {
		return await use();
	} finally {
		for (const [key, value] of Object.entries(previous)) {
			if (value === undefined) {
				delete process.env[key];
			} else {
				process.env[key] = value;
			}
		}
	}
}

function unverified_clone_destination(canonical_root: string, projects_root: string) {
	return {
		canonical_root,
		projects_root,
		projects_root_device: "0",
		projects_root_inode: "0",
		root_device: "0",
		root_inode: "0",
	};
}

async function clone_destination_proof(destination_path: string) {
	const { file_system, path_service } = await test_platform();

	return Effect.runPromise(
		Effect.scoped(
			Effect.gen(function* () {
				const canonical_root = yield* file_system.realPath(destination_path);
				const projects_root = yield* file_system.realPath(
					path_service.dirname(canonical_root),
				);
				const root_file = yield* file_system.open(canonical_root, { flag: "r" });
				const projects_root_file = yield* file_system.open(projects_root, { flag: "r" });
				const root_identity = yield* ReadFileIdentity(root_file.fd);
				const projects_root_identity = yield* ReadFileIdentity(projects_root_file.fd);

				return {
					canonical_root,
					projects_root,
					projects_root_device: projects_root_identity.device.toString(),
					projects_root_inode: projects_root_identity.inode.toString(),
					root_device: root_identity.device.toString(),
					root_inode: root_identity.inode.toString(),
				};
			}),
		),
	);
}

async function with_clone_destination<A>(
	use: (
		destination_path: string,
		destination: Awaited<ReturnType<typeof clone_destination_proof>>,
	) => Promise<A>,
) {
	return with_temporary_directory(async (root) => {
		const { file_system, path_service } = await test_platform();
		const destination_path = path_service.join(root, "editor");

		await Effect.runPromise(file_system.makeDirectory(destination_path));

		return use(destination_path, await clone_destination_proof(destination_path));
	});
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
							Layer.mergeAll(
								Layer.succeed(GitHubCliExecutable, {
									Locate: Effect.succeed(Option.none()),
								}),
								Layer.succeed(GitHubCliGitExecutable, {
									Locate: Effect.succeed(
										Option.some({ path: git_executable_path }),
									),
								}),
							),
						),
						Layer.provide(
							Layer.succeed(ProcessRunner, {
								Run: () => {
									spawn_count += 1;
									return Effect.succeed(process_result(""));
								},
							}),
						),
						Layer.provide(platform_layer),
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
		expect(calls.every((input) => input.environment_mode === "replace")).toBe(true);

		for (const call of calls) {
			expect(call.environment).toMatchObject({
				GH_NO_UPDATE_NOTIFIER: "1",
				GH_PAGER: "cat",
				GH_PROMPT_DISABLED: "1",
				GIT_CONFIG_GLOBAL: process.platform === "win32" ? "NUL" : "/dev/null",
				GIT_CONFIG_NOSYSTEM: "1",
				NO_COLOR: "1",
			});
			expect(call.environment).not.toHaveProperty("GH_TOKEN");
			expect(call.environment).not.toHaveProperty("GH_ENTERPRISE_TOKEN");
		}

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

	it("inspects one exact repository without extracting an account token", async () => {
		const calls: Array<ProcessRunnerInput> = [];
		const cli = await make_cli((input) => {
			calls.push(input);

			return Effect.succeed(
				process_result(
					JSON.stringify({
						data: {
							repository: repository("artisan/editor", "main", "WRITE"),
							viewer: { login: "alice" },
						},
					}),
				),
			);
		});

		await expect(
			Effect.runPromise(
				cli.InspectRepository({
					account_login: "alice",
					host: "ghe.example",
					name: "editor",
					owner: "artisan",
				}),
			),
		).resolves.toMatchObject({
			repository: { name: "editor", native_id: "id-editor" },
			viewer_login: "alice",
		});
		expect(calls).toHaveLength(1);
		expect(calls[0]?.args.slice(0, 6)).toEqual([
			"api",
			"graphql",
			"--hostname",
			"ghe.example",
			"--method",
			"POST",
		]);
		expect(calls[0]?.args).toContain("owner=artisan");
		expect(calls[0]?.args).toContain("name=editor");
		expect(calls[0]?.environment).toMatchObject({ GH_HOST: "ghe.example" });
		expect(calls[0]?.environment).not.toHaveProperty("GH_TOKEN");
		expect(calls[0]?.environment).not.toHaveProperty("GH_ENTERPRISE_TOKEN");
	});

	it("uses bounded argv-only GraphQL reads for a branch association without token extraction", async () => {
		const calls: Array<ProcessRunnerInput> = [];
		const cli = await make_cli((input) => {
			calls.push(input);
			const association = pull_request_association("ghe.example");
			const candidate = association.repository.pullRequests.nodes[0]!;

			return Effect.succeed(
				process_result(
					JSON.stringify({
						data: {
							repository: {
								pullRequests: {
									nodes: [
										{
											...candidate,
											headRepository: {
												name: "forked-editor",
												owner: { login: "someone-else" },
											},
										},
									],
									pageInfo: { hasNextPage: false, endCursor: null },
								},
							},
							viewer: { login: "alice" },
						},
					}),
				),
			);
		});

		await expect(
			Effect.runPromise(
				cli.ReadPullRequest({
					host: "ghe.example",
					name: "editor",
					owner: "artisan",
					selected_branch: "feature/read",
				}),
			),
		).resolves.toEqual({ type: "no_pull_request", viewer_login: "alice" });
		expect(calls[0]?.args.slice(0, 6)).toEqual([
			"api",
			"graphql",
			"--hostname",
			"ghe.example",
			"--method",
			"POST",
		]);
		expect(calls[0]?.args).toContain("owner=artisan");
		expect(calls[0]?.args).toContain("name=editor");
		expect(calls[0]?.args).toContain("branch=feature/read");
		expect(calls[0]?.args.find((argument) => argument.startsWith("query="))).toContain(
			"headRepository",
		);
		expect(calls[0]?.environment).toMatchObject({ GH_HOST: "ghe.example" });
		expect(JSON.stringify(calls[0])).not.toMatch(/GH_TOKEN|GH_ENTERPRISE_TOKEN|token=/i);
	});

	it("reads one exact PR number on the selected host and rejects association/detail races", async () => {
		const calls: Array<ProcessRunnerInput> = [];
		const cli = await make_cli((input) => {
			calls.push(input);
			const data =
				calls.length === 1
					? pull_request_association("ghe.example")
					: {
							repository: {
								pullRequest: pull_request_detail("a".repeat(40), "ghe.example"),
							},
							viewer: { login: "alice" },
						};

			return Effect.succeed(process_result(JSON.stringify({ data })));
		});
		const result = await Effect.runPromise(
			cli.ReadPullRequest({
				host: "ghe.example",
				name: "editor",
				owner: "artisan",
				selected_branch: "feature/read",
			}),
		);

		expect(result).toMatchObject({
			pull_request: { headRefOid: "a".repeat(40), number: 7 },
			type: "matched_pull_request",
			viewer_login: "alice",
		});
		expect(calls).toHaveLength(2);
		expect(calls[1]?.args.slice(0, 6)).toEqual([
			"api",
			"graphql",
			"--hostname",
			"ghe.example",
			"--method",
			"POST",
		]);
		expect(calls[1]?.args).toContain("number=7");
		const detail_query = calls[1]?.args.find((argument) => argument.startsWith("query="));

		expect(detail_query).toContain("isRequired(pullRequestNumber: $number)");
		expect(detail_query).toContain("annotations(first: 50)");
		expect(detail_query).toContain("reviewRequests(first: 100)");
		expect(detail_query).not.toMatch(/\b(?:body|logs|summary|text)\b/u);
		expect(calls.every((call) => call.environment?.GH_HOST === "ghe.example")).toBe(true);
		expect(JSON.stringify(calls)).not.toMatch(/GH_TOKEN|GH_ENTERPRISE_TOKEN|token=/i);

		const raced = await make_cli((input) =>
			Effect.succeed(
				process_result(
					JSON.stringify({
						data: input.args.some((argument) => argument === "number=7")
							? {
									repository: {
										pullRequest: pull_request_detail("f".repeat(40)),
									},
									viewer: { login: "alice" },
								}
							: pull_request_association(),
					}),
				),
			),
		);
		await expect(
			Effect.runPromise(
				raced.ReadPullRequest({
					host: "github.com",
					name: "editor",
					owner: "artisan",
					selected_branch: "feature/read",
				}),
			),
		).rejects.toMatchObject({ operation: "read_pull_request", reason: "invalid_response" });
	});

	it("reads one exact pull-request target without a branch association query", async () => {
		const calls: Array<ProcessRunnerInput> = [];
		const cli = await make_cli((input) => {
			calls.push(input);

			return Effect.succeed(
				process_result(
					JSON.stringify({
						data: {
							repository: {
								pullRequest: pull_request_detail("a".repeat(40), "ghe.example"),
							},
							viewer: { login: "alice" },
						},
					}),
				),
			);
		});
		const result = await Effect.runPromise(
			cli.ReadPullRequestTarget({
				host: "ghe.example",
				name: "editor",
				owner: "artisan",
				pull_request_number: 7,
				pull_request_native_id: "pull-request-7",
				selected_branch: "feature/read",
			}),
		);

		expect(result).toMatchObject({
			pull_request: { headRefOid: "a".repeat(40), number: 7 },
			type: "matched_pull_request",
			viewer_login: "alice",
		});
		expect(calls).toHaveLength(1);
		expect(calls[0]?.args).toContain("owner=artisan");
		expect(calls[0]?.args).toContain("name=editor");
		expect(calls[0]?.args).toContain("number=7");
		expect(calls[0]?.args).not.toContain("branch=feature/read");
		expect(calls[0]?.args.find((argument) => argument.startsWith("query="))).not.toContain(
			"PullRequestAssociation",
		);
		expect(calls[0]?.environment).toMatchObject({ GH_HOST: "ghe.example" });
	});

	it("fails closed when an exact pull-request target is malformed or inconsistent", async () => {
		for (const pull_request of [
			{ ...pull_request_detail(), number: 8 },
			{ ...pull_request_detail(), id: "recreated-pull-request-7" },
			{ ...pull_request_detail(), headRefName: "other-branch" },
			{
				...pull_request_detail(),
				headRepository: { name: "other-repository", owner: { login: "artisan" } },
			},
			{ ...pull_request_detail(), unexpected: "field" },
		] as const) {
			const cli = await make_cli(() =>
				Effect.succeed(
					process_result(
						JSON.stringify({
							data: {
								repository: { pullRequest: pull_request },
								viewer: { login: "alice" },
							},
						}),
					),
				),
			);

			await expect(
				Effect.runPromise(
					cli.ReadPullRequestTarget({
						host: "github.com",
						name: "editor",
						owner: "artisan",
						pull_request_number: 7,
						pull_request_native_id: "pull-request-7",
						selected_branch: "feature/read",
					}),
				),
			).rejects.toMatchObject({
				operation: "read_pull_request_target",
				reason: "invalid_response",
			});
		}

		const truncated = await make_cli(() =>
			Effect.succeed(
				process_result(
					JSON.stringify({
						data: {
							repository: { pullRequest: pull_request_detail() },
							viewer: { login: "alice" },
						},
					}),
					{ stdout_truncated: true },
				),
			),
		);

		await expect(
			Effect.runPromise(
				truncated.ReadPullRequestTarget({
					host: "github.com",
					name: "editor",
					owner: "artisan",
					pull_request_number: 7,
					pull_request_native_id: "pull-request-7",
					selected_branch: "feature/read",
				}),
			),
		).rejects.toMatchObject({
			operation: "read_pull_request_target",
			reason: "invalid_response",
		});
	});

	it("fails closed on malformed or truncated pull-request detail output", async () => {
		for (const mode of ["malformed", "truncated"] as const) {
			let call_count = 0;
			const cli = await make_cli(() => {
				call_count += 1;
				const data =
					call_count === 1
						? pull_request_association()
						: {
								repository: {
									pullRequest: { ...pull_request_detail(), unexpected: "field" },
								},
								viewer: { login: "alice" },
							};

				return Effect.succeed(
					process_result(JSON.stringify({ data }), {
						stdout_truncated: mode === "truncated" && call_count === 2,
					}),
				);
			});

			await expect(
				Effect.runPromise(
					cli.ReadPullRequest({
						host: "github.com",
						name: "editor",
						owner: "artisan",
						selected_branch: "feature/read",
					}),
				),
				mode,
			).rejects.toMatchObject({ operation: "read_pull_request", reason: "invalid_response" });
		}
	});

	it("classifies pull-request rate limits and network failures without retaining output", async () => {
		const cases = [
			{
				result: process_result(
					JSON.stringify({ errors: [{ message: "API rate limit exceeded secret" }] }),
					{ exit_code: 1 },
				),
				reason: "rate_limited",
			},
			{
				result: process_result("", {
					exit_code: 1,
					stderr: Buffer.from("failed to connect private-host-detail"),
				}),
				reason: "network_unavailable",
			},
		] as const;

		for (const test_case of cases) {
			const cli = await make_cli(() => Effect.succeed(test_case.result));
			const error = await Effect.runPromise(
				cli
					.ReadPullRequest({
						host: "github.com",
						name: "editor",
						owner: "artisan",
						selected_branch: "feature/read",
					})
					.pipe(Effect.flip),
			);

			expect(error).toMatchObject({
				operation: "read_pull_request",
				reason: test_case.reason,
				retryable: true,
			});
			expect(JSON.stringify(error)).not.toMatch(/secret|private-host-detail/);
		}
	});

	it("distinguishes a missing pinned Git executable without spawning", async () => {
		let spawn_count = 0;
		const cli = await make_cli(
			() => {
				spawn_count += 1;

				return Effect.succeed(process_result(""));
			},
			{},
			{ git_path: null },
		);
		const error = await Effect.runPromise(
			cli
				.CloneRepository({
					account_login: "alice",
					destination: unverified_clone_destination(
						"C:\\Projects\\editor",
						"C:\\Projects",
					),
					host: "github.com",
					name: "editor",
					owner: "artisan",
				})
				.pipe(Effect.scoped, Effect.flip),
		);

		expect(error).toMatchObject({
			operation: "clone_repository",
			reason: "git_dependency_missing",
			retryable: false,
		});
		expect(spawn_count).toBe(0);
	});

	it("proves a real checkout and brokers selected-account credentials child to child", async () =>
		with_temporary_directory(async (root) => {
			const { file_system, path_service } = await test_platform();
			const real_runner = await Effect.runPromise(
				Effect.service(ProcessRunner).pipe(Effect.provide(NodeProcessRunnerLive)),
			);
			const source_path = path_service.join(root, "source.git");
			const destination_path = path_service.join(root, "editor");
			const credential_log_path = path_service.join(root, "credential-log.jsonl");
			const auth_script_path = path_service.join(root, "auth");
			const ambient_home = path_service.join(root, "ambient-home");
			const ambient_gh_config = path_service.join(root, "ambient-gh-config");
			const ambient_netrc = path_service.join(ambient_home, ".netrc");
			const expected_url = "https://github.com/artisan/editor.git";
			const calls: Array<ProcessRunnerInput> = [];
			let clone_call: ProcessRunnerInput | undefined;
			let template_path: string | undefined;

			await Effect.runPromise(
				Effect.all([
					file_system.makeDirectory(ambient_home),
					file_system.makeDirectory(ambient_gh_config),
					file_system.makeDirectory(destination_path),
				]),
			);
			const destination = await clone_destination_proof(destination_path);
			await Effect.runPromise(
				Effect.all([
					file_system.writeFileString(
						ambient_netrc,
						"machine github.com login ambient password ambient-netrc-token\n",
					),
					file_system.writeFileString(
						path_service.join(ambient_home, ".gitconfig"),
						"[credential]\n\thelper = store\n",
					),
					file_system.writeFileString(
						path_service.join(ambient_home, ".git-credentials"),
						"https://ambient:ambient-store-token@github.com\n",
					),
				]),
			);
			await Effect.runPromise(
				file_system.writeFileString(
					auth_script_path,
					[
						'const { appendFileSync } = require("node:fs");',
						`appendFileSync(${JSON.stringify(credential_log_path)}, JSON.stringify({ args: process.argv.slice(2), gh_config_dir: process.env.GH_CONFIG_DIR }) + "\\n");`,
						'process.stdout.write("selected-account-token\\n");',
					].join("\n"),
					{ mode: 0o700 },
				),
			);

			const initialized = await Effect.runPromise(
				real_runner.Run({
					args: ["init", "--bare", "--quiet", source_path],
					command: git_executable_path,
					cwd: root,
					max_stderr_bytes: 64 * 1024,
					max_stdout_bytes: 64 * 1024,
				}),
			);

			expect(initialized.exit_code).toBe(0);

			const cli = await with_process_environment(
				{
					APPDATA: path_service.join(ambient_home, "appdata"),
					CURL_HOME: ambient_home,
					GH_CONFIG_DIR: ambient_gh_config,
					HOME: ambient_home,
					NETRC: ambient_netrc,
					USERPROFILE: ambient_home,
					XDG_CONFIG_HOME: path_service.join(ambient_home, "xdg"),
				},
				() =>
					make_cli(
						(input) => {
							calls.push(input);

							if (
								input.command !== git_executable_path ||
								input.args[0] !== "clone"
							) {
								return real_runner.Run(input);
							}

							clone_call = input;
							template_path = input.args
								.find((argument) => argument.startsWith("--template="))
								?.slice("--template=".length);

							const separator = input.args.indexOf("--");

							if (
								separator < 0 ||
								input.args[separator + 2] !== destination_path ||
								input.environment === undefined
							) {
								return Effect.die(
									"Clone command did not retain its exact destination",
								);
							}

							const clone_environment = input.environment;
							const local_arguments = input.args.map((argument, index) =>
								index === separator + 1 ? source_path : argument,
							);

							return real_runner.Run({ ...input, args: local_arguments }).pipe(
								Effect.flatMap((result) =>
									result.exit_code !== 0
										? Effect.succeed(result)
										: real_runner
												.Run({
													args: [
														"-C",
														destination_path,
														"config",
														"--local",
														"remote.origin.url",
														expected_url,
													],
													command: git_executable_path,
													cwd: root,
													environment: clone_environment,
													environment_mode: "replace",
													max_stderr_bytes: 64 * 1024,
													max_stdout_bytes: 64 * 1024,
												})
												.pipe(
													Effect.flatMap((configured) =>
														configured.exit_code === 0
															? Effect.succeed(result)
															: Effect.die(
																	"Fixture could not restore the approved origin",
																),
													),
												),
								),
							);
						},
						{ cwd: root },
						{ gh_path: process.execPath },
					),
			);
			const result = await Effect.runPromise(
				Effect.gen(function* () {
					const execution = yield* cli.CloneRepository({
						account_login: "alice",
						destination,
						host: "github.com",
						name: "editor",
						owner: "artisan",
					});

					yield* execution.VerifyCheckout;

					return {
						canonical_root: execution.canonical_root,
						output_complete: execution.output_complete,
					};
				}).pipe(Effect.scoped),
			);
			const canonical_root = await Effect.runPromise(file_system.realPath(destination_path));

			if (
				clone_call === undefined ||
				template_path === undefined ||
				clone_call.environment === undefined
			) {
				throw new Error("Clone execution was not captured");
			}

			const clone_environment = clone_call.environment;

			expect(result).toEqual({ canonical_root, output_complete: true });
			expect(clone_call.args).toEqual([
				"clone",
				`--template=${template_path}`,
				"--no-recurse-submodules",
				"--origin=origin",
				"--",
				expected_url,
				canonical_root,
			]);
			expect(clone_call.command).toBe(git_executable_path);
			expect(clone_call.cwd).toBe(root);
			expect(clone_environment).toMatchObject({
				ARTISAN_GH_ACCOUNT: "alice",
				ARTISAN_GH_CONFIG_DIR: ambient_gh_config,
				ARTISAN_GH_EXECUTABLE:
					process.platform === "win32"
						? process.execPath.replaceAll("\\", "/")
						: process.execPath,
				ARTISAN_GH_HOST: "github.com",
				APPDATA: path_service.join(template_path, "appdata"),
				GH_CONFIG_DIR: undefined,
				GIT_CONFIG_COUNT: "4",
				GIT_CONFIG_KEY_0: "core.hooksPath",
				GIT_CONFIG_KEY_1: "credential.helper",
				GIT_CONFIG_KEY_2: "credential.helper",
				GIT_CONFIG_KEY_3: "http.followRedirects",
				GIT_CONFIG_VALUE_0: process.platform === "win32" ? "NUL" : "/dev/null",
				GIT_CONFIG_VALUE_1: "",
				GIT_CONFIG_VALUE_3: "false",
				GIT_TERMINAL_PROMPT: "0",
				HOME: template_path,
				HOMEDRIVE: undefined,
				HOMEPATH: undefined,
				NETRC: path_service.join(template_path, ".netrc"),
				PATH: dirname(git_executable_path),
				USERPROFILE: template_path,
				XDG_CONFIG_HOME: path_service.join(template_path, "xdg"),
			});
			expect(clone_environment).not.toHaveProperty("CURL_HOME");
			expect(clone_environment.NETRC).not.toBe(ambient_netrc);
			expect(clone_environment.GIT_CONFIG_VALUE_2).toContain(
				'"$ARTISAN_GH_EXECUTABLE" auth token',
			);
			expect(clone_environment).not.toHaveProperty("GH_TOKEN");
			expect(clone_environment).not.toHaveProperty("GH_ENTERPRISE_TOKEN");
			expect(clone_call.args.join(" ")).not.toMatch(/token|credential|password/iu);
			expect(await Effect.runPromise(file_system.exists(template_path))).toBe(false);
			expect(
				await Effect.runPromise(
					file_system.readFileString(
						path_service.join(destination_path, ".git", "artisan-clone-receipt"),
					),
				),
			).toMatch(/^[0-9a-f-]{36}\n$/u);

			const credential = await Effect.runPromise(
				real_runner.Run({
					args: ["credential", "fill"],
					command: git_executable_path,
					cwd: root,
					environment: clone_environment,
					environment_mode: "replace",
					max_stderr_bytes: 64 * 1024,
					max_stdout_bytes: 64 * 1024,
					stdin: Buffer.from("protocol=https\nhost=github.com\n\n"),
				}),
			);

			expect(credential.exit_code).toBe(0);
			expect(Buffer.from(credential.stdout).toString("utf8")).toContain(
				"username=x-access-token\npassword=selected-account-token\n",
			);
			expect(Buffer.from(credential.stdout).toString("utf8")).not.toMatch(
				/ambient-(?:netrc|store)-token/u,
			);
			expect(await Effect.runPromise(file_system.readFileString(credential_log_path))).toBe(
				`${JSON.stringify({
					args: ["token", "--hostname", "github.com", "--user", "alice"],
					gh_config_dir: clone_environment.ARTISAN_GH_CONFIG_DIR,
				})}\n`,
			);

			const ignored = await Effect.runPromise(
				real_runner.Run({
					args: ["credential", "approve"],
					command: git_executable_path,
					cwd: root,
					environment: clone_environment,
					environment_mode: "replace",
					max_stderr_bytes: 64 * 1024,
					max_stdout_bytes: 64 * 1024,
					stdin: Buffer.from(
						"protocol=https\nhost=github.com\nusername=x-access-token\npassword=ignored\n\n",
					),
				}),
			);
			const wrong_host = await Effect.runPromise(
				real_runner.Run({
					args: ["credential", "fill"],
					command: git_executable_path,
					cwd: root,
					environment: clone_environment,
					environment_mode: "replace",
					max_stderr_bytes: 64 * 1024,
					max_stdout_bytes: 64 * 1024,
					stdin: Buffer.from("protocol=https\nhost=elsewhere.example\n\n"),
				}),
			);
			const wrong_protocol = await Effect.runPromise(
				real_runner.Run({
					args: ["credential", "fill"],
					command: git_executable_path,
					cwd: root,
					environment: clone_environment,
					environment_mode: "replace",
					max_stderr_bytes: 64 * 1024,
					max_stdout_bytes: 64 * 1024,
					stdin: Buffer.from("protocol=http\nhost=github.com\n\n"),
				}),
			);

			expect(ignored.exit_code).toBe(0);
			expect(wrong_host.exit_code).not.toBe(0);
			expect(wrong_protocol.exit_code).not.toBe(0);
			expect(await Effect.runPromise(file_system.readFileString(credential_log_path))).toBe(
				`${JSON.stringify({
					args: ["token", "--hostname", "github.com", "--user", "alice"],
					gh_config_dir: clone_environment.ARTISAN_GH_CONFIG_DIR,
				})}\n`,
			);
		}));

	it("rejects a missing destination before spawning Git", async () => {
		await with_temporary_directory(async (root) => {
			const { path_service } = await test_platform();
			let spawn_count = 0;
			const cli = await make_cli(
				() => {
					spawn_count += 1;

					return Effect.succeed(process_result(""));
				},
				{ cwd: root },
			);
			const error = await Effect.runPromise(
				cli
					.CloneRepository({
						account_login: "alice",
						destination: unverified_clone_destination(
							path_service.join(root, "missing"),
							root,
						),
						host: "github.com",
						name: "editor",
						owner: "artisan",
					})
					.pipe(Effect.scoped, Effect.flip),
			);

			expect(error).toMatchObject({
				operation: "clone_repository",
				reason: "invalid_destination",
				retryable: false,
			});
			expect(spawn_count).toBe(0);
		});
	});

	it("rejects an empty destination outside the configured projects root", async () => {
		await with_temporary_directory(async (root) => {
			const { file_system, path_service } = await test_platform();
			const projects_root = path_service.join(root, "projects");
			const destination_path = path_service.join(root, "outside");
			let spawn_count = 0;

			await Effect.runPromise(file_system.makeDirectory(projects_root));
			await Effect.runPromise(file_system.makeDirectory(destination_path));

			const cli = await make_cli(
				() => {
					spawn_count += 1;

					return Effect.succeed(process_result(""));
				},
				{ cwd: root, projects_root },
			);
			const destination = await clone_destination_proof(destination_path);
			const error = await Effect.runPromise(
				cli
					.CloneRepository({
						account_login: "alice",
						destination,
						host: "github.com",
						name: "editor",
						owner: "artisan",
					})
					.pipe(Effect.scoped, Effect.flip),
			);

			expect(error).toMatchObject({
				operation: "clone_repository",
				reason: "invalid_destination",
				retryable: false,
			});
			expect(spawn_count).toBe(0);
		});
	});

	it("rejects a destination replaced after approval before spawning Git", async () => {
		await with_clone_destination(async (destination_path, destination) => {
			const { file_system } = await test_platform();
			let spawn_count = 0;

			await Effect.runPromise(file_system.remove(destination_path, { recursive: true }));
			await Effect.runPromise(file_system.makeDirectory(destination_path));

			const cli = await make_cli(
				() => {
					spawn_count += 1;

					return Effect.succeed(process_result(""));
				},
				{ cwd: dirname(destination_path) },
			);
			const error = await Effect.runPromise(
				cli
					.CloneRepository({
						account_login: "alice",
						destination,
						host: "github.com",
						name: "editor",
						owner: "artisan",
					})
					.pipe(Effect.scoped, Effect.flip),
			);

			expect(error).toMatchObject({
				operation: "clone_repository",
				reason: "invalid_destination",
				retryable: false,
			});
			expect(spawn_count).toBe(0);
		});
	});

	it("rejects a mismatched approved projects-root identity before spawning Git", async () => {
		await with_clone_destination(async (destination_path, destination) => {
			let spawn_count = 0;
			const cli = await make_cli(
				() => {
					spawn_count += 1;

					return Effect.succeed(process_result(""));
				},
				{ cwd: dirname(destination_path) },
			);
			const error = await Effect.runPromise(
				cli
					.CloneRepository({
						account_login: "alice",
						destination: {
							...destination,
							projects_root_inode:
								destination.projects_root_inode === "0" ? "1" : "0",
						},
						host: "github.com",
						name: "editor",
						owner: "artisan",
					})
					.pipe(Effect.scoped, Effect.flip),
			);

			expect(error).toMatchObject({
				operation: "clone_repository",
				reason: "invalid_destination",
				retryable: false,
			});
			expect(spawn_count).toBe(0);
		});
	});

	it("quarantines a destination replacement after Git reports success", async () => {
		await with_clone_destination(async (destination_path, destination) => {
			const { file_system } = await test_platform();
			let spawn_count = 0;
			const cli = await make_cli(
				() => {
					spawn_count += 1;

					return file_system
						.remove(destination_path, { recursive: true })
						.pipe(
							Effect.andThen(file_system.makeDirectory(destination_path)),
							Effect.as(process_result("")),
							Effect.orDie,
						);
				},
				{ cwd: dirname(destination_path) },
			);
			const error = await Effect.runPromise(
				cli
					.CloneRepository({
						account_login: "alice",
						destination,
						host: "github.com",
						name: "editor",
						owner: "artisan",
					})
					.pipe(Effect.scoped, Effect.flip),
			);

			expect(error).toMatchObject({
				operation: "clone_repository",
				reason: "outcome_unknown",
				retryable: false,
			});
			expect(spawn_count).toBe(1);
		});
	});

	it("quarantines every nonzero clone exit without retaining provider output", async () => {
		await with_clone_destination(async (destination_path, destination) => {
			const raw_output = "fatal: authentication failed secret-provider-output";
			const cli = await make_cli(
				() =>
					Effect.succeed(
						process_result("", {
							exit_code: 1,
							stderr: Buffer.from(raw_output),
						}),
					),
				{ cwd: dirname(destination_path) },
			);
			const error = await Effect.runPromise(
				cli
					.CloneRepository({
						account_login: "alice",
						destination,
						host: "github.com",
						name: "editor",
						owner: "artisan",
					})
					.pipe(Effect.scoped, Effect.flip),
			);

			expect(error).toMatchObject({
				operation: "clone_repository",
				reason: "outcome_unknown",
				retryable: false,
			});
			expect(JSON.stringify(error)).not.toContain(raw_output);
		});
	});

	it("quarantines a clone timeout as an unknown outcome", async () => {
		await with_clone_destination(async (destination_path, destination) => {
			const cli = await make_cli(() => Effect.never, {
				clone_timeout_ms: 1,
				cwd: dirname(destination_path),
			});
			const error = await Effect.runPromise(
				cli
					.CloneRepository({
						account_login: "alice",
						destination,
						host: "github.com",
						name: "editor",
						owner: "artisan",
					})
					.pipe(Effect.scoped, Effect.flip),
			);

			expect(error).toMatchObject({
				operation: "clone_repository",
				reason: "outcome_unknown",
				retryable: false,
			});
		});
	});

	it("converts external clone cancellation into an unknown outcome", async () => {
		await with_clone_destination(async (destination_path, destination) => {
			const started = await Effect.runPromise(Deferred.make<void>());
			const cli = await make_cli(
				() => Deferred.succeed(started, undefined).pipe(Effect.andThen(Effect.never)),
				{ cwd: dirname(destination_path) },
			);
			const error = await Effect.runPromise(
				Effect.gen(function* () {
					const fiber = yield* Effect.forkChild(
						cli.CloneRepository({
							account_login: "alice",
							destination,
							host: "github.com",
							name: "editor",
							owner: "artisan",
						}),
					);

					yield* Deferred.await(started);
					yield* Fiber.interrupt(fiber);
					const exit = yield* Fiber.await(fiber);

					return Exit.isFailure(exit)
						? Cause.squash(exit.cause)
						: yield* Effect.die("Cancellation must fail the clone");
				}).pipe(Effect.scoped),
			);

			expect(error).toMatchObject({
				operation: "clone_repository",
				reason: "outcome_unknown",
				retryable: false,
			});
		});
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

function pull_request_association(host = "github.com") {
	return {
		repository: {
			pullRequests: {
				nodes: [
					{
						baseRefName: "main",
						headRefName: "feature/read",
						headRefOid: "a".repeat(40),
						headRepository: { name: "editor", owner: { login: "artisan" } },
						id: "pull-request-7",
						isDraft: false,
						isMerged: false,
						number: 7,
						state: "OPEN",
						title: "Add hosted reads",
						url: `https://${host}/artisan/editor/pull/7`,
					},
				],
				pageInfo: { endCursor: null, hasNextPage: false },
			},
		},
		viewer: { login: "alice" },
	};
}

function pull_request_detail(head_ref_oid = "a".repeat(40), host = "github.com") {
	return {
		baseRefName: "main",
		baseRefOid: "b".repeat(40),
		commits: { nodes: [{ commit: { statusCheckRollup: null } }] },
		headRefName: "feature/read",
		headRefOid: head_ref_oid,
		headRepository: { name: "editor", owner: { login: "artisan" } },
		id: "pull-request-7",
		isDraft: false,
		isMerged: false,
		mergeable: "MERGEABLE",
		number: 7,
		requestedReviewers: {
			nodes: [],
			pageInfo: { endCursor: null, hasNextPage: false },
			totalCount: 0,
		},
		reviewDecision: null,
		reviewThreads: {
			nodes: [],
			pageInfo: { endCursor: null, hasNextPage: false },
			totalCount: 0,
		},
		reviews: { nodes: [], pageInfo: { endCursor: null, hasNextPage: false }, totalCount: 0 },
		state: "OPEN",
		title: "Add hosted reads",
		url: `https://${host}/artisan/editor/pull/7`,
	};
}

function check_failure_detail(overrides: Record<string, unknown> = {}) {
	return {
		checkRun: {
			__typename: "CheckRun",
			checkSuite: {
				commit: {
					oid: "a".repeat(40),
					repository: { id: "repository-1", nameWithOwner: "artisan/editor" },
				},
				id: "suite-1",
				workflowRun: { databaseId: 101, id: "run-1", runAttempt: 2 },
			},
			completedAt: "2026-07-14T10:05:00Z",
			databaseId: 202,
			id: "check-1",
			name: "build",
			summary: "summary",
			status: "COMPLETED",
			text: "text",
			title: "failure",
		},
		repository: {
			id: "repository-1",
			nameWithOwner: "artisan/editor",
			pullRequest: {
				headRefName: "feature/read",
				headRefOid: "a".repeat(40),
				headRepository: { name: "editor", owner: { login: "artisan" } },
				id: "pull-request-7",
				number: 7,
			},
		},
		viewer: { login: "alice" },
		...overrides,
	};
}

describe("GitHubCli check failure detail", () => {
	it("binds the check to the exact repository, PR, branch, head, and Actions job log", async () => {
		const calls: Array<ProcessRunnerInput> = [];
		const cli = await make_cli((input) => {
			calls.push(input);

			return Effect.succeed(
				input.args[0] === "api"
					? process_result(JSON.stringify({ data: check_failure_detail() }))
					: process_result("failed\u0000 job\nnext line", {
							stdout_bytes: 70_000,
							stdout_truncated: true,
						}),
			);
		});
		const result = await Effect.runPromise(
			cli.ReadCheckFailureDetail({
				check_native_id: "check-1",
				expected_head: "a".repeat(40),
				host: "ghe.example",
				name: "editor",
				owner: "artisan",
				pull_request_native_id: "pull-request-7",
				pull_request_number: 7,
				selected_branch: "feature/read",
			}),
		);

		expect(result).toMatchObject({
			attempt: 2,
			check_native_id: "check-1",
			log: {
				observed_bytes: 70_000,
				truncated: true,
				untrusted_excerpt: "failed job\nnext line",
			},
			workflow_native_id: "run-1",
		});
		expect(calls[0]?.args.find((argument) => argument.startsWith("query="))).toContain(
			"databaseId",
		);
		expect(calls[1]?.args).toEqual(
			expect.arrayContaining([
				"--repo",
				"ghe.example/artisan/editor",
				"--attempt",
				"2",
				"--job",
				"202",
				"--log-failed",
			]),
		);
		expect(calls[1]?.max_stdout_bytes).toBe(64 * 1024);
	});

	it("truncates canonical text and skips logs without one complete Actions job identity", async () => {
		for (const fixture of [
			{
				check: {
					...check_failure_detail().checkRun,
					completedAt: null,
					status: "IN_PROGRESS",
				},
				reason: "check_not_completed",
			},
			{
				check: {
					...check_failure_detail().checkRun,
					checkSuite: {
						...check_failure_detail().checkRun.checkSuite,
						workflowRun: null,
					},
				},
				reason: "not_actions_job",
			},
			{
				check: {
					...check_failure_detail().checkRun,
					checkSuite: {
						...check_failure_detail().checkRun.checkSuite,
						workflowRun: {
							...check_failure_detail().checkRun.checkSuite.workflowRun,
							databaseId: null,
						},
					},
				},
				reason: "not_available",
			},
		] as const) {
			const check = fixture.check;
			let calls = 0;
			const cli = await make_cli(() => {
				calls += 1;
				return Effect.succeed(
					process_result(
						JSON.stringify({
							data: check_failure_detail({
								checkRun: {
									...check,
									summary: "\u0001" + "🙂".repeat(2_000),
									text: null,
									title: "\u0002title",
								},
							}),
						}),
					),
				);
			});
			const result = await Effect.runPromise(
				cli.ReadCheckFailureDetail({
					check_native_id: "check-1",
					expected_head: "a".repeat(40),
					host: "github.com",
					name: "editor",
					owner: "artisan",
					pull_request_native_id: "pull-request-7",
					pull_request_number: 7,
					selected_branch: "feature/read",
				}),
			);

			expect(result.output.summary).toMatchObject({ truncated: true });
			expect(result.log).toEqual({ _tag: "unavailable", reason: fixture.reason });
			expect(JSON.stringify(result)).not.toMatch(/[\p{Cc}\p{Cf}]/u);
			expect(calls).toBe(1);
		}
	});

	it("keeps bounded check output when the failed-job log artifact is absent", async () => {
		let calls = 0;
		const cli = await make_cli(() => {
			calls += 1;

			return Effect.succeed(
				calls === 1
					? process_result(JSON.stringify({ data: check_failure_detail() }))
					: process_result("", {
							exit_code: 1,
							stderr: Buffer.from("HTTP 404: log not found"),
						}),
			);
		});
		const result = await Effect.runPromise(
			cli.ReadCheckFailureDetail({
				check_native_id: "check-1",
				expected_head: "a".repeat(40),
				host: "github.com",
				name: "editor",
				owner: "artisan",
				pull_request_native_id: "pull-request-7",
				pull_request_number: 7,
				selected_branch: "feature/read",
			}),
		);

		expect(result.log).toEqual({ _tag: "unavailable", reason: "not_available" });
		expect(result.output.summary).toMatchObject({ untrusted_text: "summary" });
		expect(calls).toBe(2);
	});

	it("rejects malformed or changed check identity before publishing output", async () => {
		for (const data of [
			check_failure_detail({
				checkRun: { ...check_failure_detail().checkRun, id: "other-check" },
			}),
			check_failure_detail({
				repository: {
					...check_failure_detail().repository,
					nameWithOwner: "other/editor",
					pullRequest: {
						...check_failure_detail().repository.pullRequest,
						headRefOid: "b".repeat(40),
						id: "recreated-pull-request",
					},
				},
			}),
			check_failure_detail({
				checkRun: {
					...check_failure_detail().checkRun,
					summary: "x".repeat(256 * 1024 + 1),
				},
			}),
			check_failure_detail({ unexpected: true }),
		] as const) {
			const cli = await make_cli(() =>
				Effect.succeed(process_result(JSON.stringify({ data }))),
			);

			await expect(
				Effect.runPromise(
					cli.ReadCheckFailureDetail({
						check_native_id: "check-1",
						expected_head: "a".repeat(40),
						host: "github.com",
						name: "editor",
						owner: "artisan",
						pull_request_native_id: "pull-request-7",
						pull_request_number: 7,
						selected_branch: "feature/read",
					}),
				),
			).rejects.toMatchObject({
				operation: "read_check_failure_detail",
				reason: "invalid_response",
			});
		}
	});
});
