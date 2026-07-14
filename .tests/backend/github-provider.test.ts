import { Cause, Deferred, Effect, Exit, Fiber, Layer, Ref, Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	GitProvider,
	GitProviderCloneExecution,
	type GitProviderDiscovery,
} from "../../modules/backend/src/git-provider/git-provider";
import {
	GitHubCli,
	GitHubCliError,
	type GitHubCliCloneResult,
	type GitHubCliInspection,
	type GitHubCliPullRequestReadResult,
	type GitHubCliRepositoryInspectionResult,
	type GitHubCliRepositoryPage,
} from "../../modules/backend/src/git-provider/github/github-cli";
import { make_github_provider_layer } from "../../modules/backend/src/git-provider/github/github-provider";

const executable_path = "C:\\Program Files\\GitHub CLI\\gh.exe";
const selection = {
	account_login: "alice",
	host: "github.com",
	provider_id: "github",
} as const;

function clone_destination() {
	return {
		canonical_root: "C:\\Projects\\editor",
		projects_root: "C:\\Projects",
		projects_root_device: "1",
		projects_root_inode: "2",
		root_device: "1",
		root_inode: "3",
	};
}

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

function projected_repository(name: string, default_branch?: string) {
	return {
		archived: true,
		clone_url: `https://github.com/artisan/${name}.git`,
		default_branch:
			default_branch === undefined
				? ({ _tag: "unavailable" } as const)
				: ({ _tag: "known", name: default_branch } as const),
		identity: {
			host: "github.com",
			name,
			owner: "artisan",
			provider_id: "github",
		},
		origin: {
			native_id: `node-${name}`,
			provider_id: "github",
			resource_kind: "repository" as const,
		},
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

function matched_pull_request(head_ref_oid = "a".repeat(40)) {
	return {
		baseRefName: "main",
		baseRefOid: "b".repeat(40),
		commits: {
			nodes: [
				{
					commit: {
						statusCheckRollup: {
							contexts: {
								nodes: [
									{
										__typename: "CheckRun",
										annotations: {
											nodes: [
												{
													annotationLevel: "FAILURE",
													location: {
														end: { line: 8 },
														start: { line: 7 },
													},
													message: "untrusted failure",
													path: "src/editor.ts",
													title: "Compile failed",
												},
											],
											pageInfo: {
												endCursor: "annotation-cursor",
												hasNextPage: true,
											},
											totalCount: 51,
										},
										checkSuite: {
											app: { name: "GitHub Actions" },
											id: "suite-1",
											workflowRun: {
												id: "run-1",
												runAttempt: 2,
												url: "https://github.com/artisan/editor/actions/runs/1",
												workflow: { name: "CI" },
											},
										},
										completedAt: "2026-07-14T10:05:00Z",
										conclusion: "FAILURE",
										detailsUrl:
											"https://github.com/artisan/editor/actions/runs/1/job/1",
										id: "check-1",
										isRequired: true,
										name: "build",
										startedAt: "2026-07-14T10:00:00Z",
										status: "COMPLETED",
									},
									{
										__typename: "StatusContext",
										context: "deploy",
										id: "status-1",
										isRequired: false,
										state: "PENDING",
										targetUrl: null,
									},
									{
										__typename: "StatusContext",
										context: "lint",
										id: "status-2",
										isRequired: true,
										state: "SUCCESS",
										targetUrl: "https://ci.example/lint",
									},
								],
								pageInfo: { endCursor: null, hasNextPage: false },
								totalCount: 3,
							},
						},
					},
				},
			],
		},
		headRefName: "feature/read",
		headRefOid: head_ref_oid,
		headRepository: { name: "editor", owner: { login: "artisan" } },
		id: "pull-request-7",
		isDraft: false,
		isMerged: false,
		mergeable: "MERGEABLE",
		number: 7,
		requestedReviewers: {
			nodes: [
				{ requestedReviewer: { __typename: "User", login: "bob" } },
				{
					requestedReviewer: {
						__typename: "Team",
						organization: { login: "artisan" },
						slug: "maintainers",
					},
				},
			],
			pageInfo: { endCursor: null, hasNextPage: false },
			totalCount: 2,
		},
		reviewDecision: "CHANGES_REQUESTED",
		reviewThreads: {
			nodes: [
				{
					comments: {
						nodes: [{ id: "comment-2", updatedAt: "2026-07-14T09:30:00Z" }],
						totalCount: 2,
					},
					id: "thread-1",
					isOutdated: true,
					isResolved: false,
					line: 17,
					path: "src/editor.ts",
					subjectType: "LINE",
				},
			],
			pageInfo: { endCursor: "thread-cursor", hasNextPage: true },
			totalCount: 101,
		},
		reviews: {
			nodes: [
				{
					author: { login: "carol" },
					commit: { oid: head_ref_oid },
					id: "review-1",
					state: "CHANGES_REQUESTED",
					submittedAt: "2026-07-14T09:00:00Z",
				},
				{
					author: null,
					commit: null,
					id: "review-2",
					state: "DISMISSED",
					submittedAt: "2026-07-14T09:10:00Z",
				},
			],
			pageInfo: { endCursor: null, hasNextPage: false },
			totalCount: 2,
		},
		state: "OPEN",
		title: "Add hosted reads",
		url: "https://github.com/artisan/editor/pull/7",
	} satisfies Extract<
		GitHubCliPullRequestReadResult,
		{ readonly type: "matched_pull_request" }
	>["pull_request"];
}

async function make_provider(options: {
	readonly inspection: GitHubCliInspection;
	readonly query?: (
		input: Parameters<(typeof GitHubCli.Service)["QueryRepositories"]>[0],
	) => Effect.Effect<GitHubCliRepositoryPage, GitHubCliError>;
	readonly clone?: (
		input: Parameters<(typeof GitHubCli.Service)["CloneRepository"]>[0],
	) => Effect.Effect<GitHubCliCloneResult, GitHubCliError>;
	readonly inspect_repository?: (
		input: Parameters<(typeof GitHubCli.Service)["InspectRepository"]>[0],
	) => Effect.Effect<GitHubCliRepositoryInspectionResult, GitHubCliError>;
	readonly read_pull_request?: (
		input: Parameters<(typeof GitHubCli.Service)["ReadPullRequest"]>[0],
	) => ReturnType<(typeof GitHubCli.Service)["ReadPullRequest"]>;
	readonly read_pull_request_target?: (
		input: Parameters<(typeof GitHubCli.Service)["ReadPullRequestTarget"]>[0],
	) => ReturnType<(typeof GitHubCli.Service)["ReadPullRequestTarget"]>;
	readonly read_check_failure_detail?: (
		input: Parameters<(typeof GitHubCli.Service)["ReadCheckFailureDetail"]>[0],
	) => ReturnType<(typeof GitHubCli.Service)["ReadCheckFailureDetail"]>;
	readonly hosts?: ReadonlyArray<string>;
}) {
	const query = options.query ?? (() => Effect.die("Unexpected repository query"));
	const clone = options.clone ?? (() => Effect.die("Unexpected repository clone"));
	const inspect_repository =
		options.inspect_repository ?? (() => Effect.die("Unexpected repository inspection"));
	const read_pull_request =
		options.read_pull_request ?? (() => Effect.die("Unexpected pull request read"));
	const read_pull_request_target =
		options.read_pull_request_target ??
		(() => Effect.die("Unexpected pull request target read"));
	const read_check_failure_detail =
		options.read_check_failure_detail ?? (() => Effect.die("Unexpected check failure read"));
	const cli_layer = Layer.succeed(GitHubCli, {
		CloneRepository: clone,
		Inspect: Effect.succeed(options.inspection),
		InspectRepository: inspect_repository,
		QueryRepositories: query,
		ReadCheckFailureDetail: read_check_failure_detail,
		ReadPullRequest: read_pull_request,
		ReadPullRequestTarget: read_pull_request_target,
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

	it("matches GitHub account identity case-insensitively", async () => {
		const provider = await make_provider({
			inspection: available_inspection([
				{
					accounts: [
						{
							active: true,
							git_protocol: "https",
							host: "github.com",
							login: "alice",
							scopes: [],
							type: "authenticated",
						},
					],
					host: "github.com",
				},
			]),
			query: () => Effect.succeed(page("ALICE", [repository("editor")])),
		});
		const result = await Effect.runPromise(
			provider.DiscoverRepositories(
				discovery({ selection: { ...selection, account_login: "Alice" } }),
			),
		);

		expect(result.repositories).toHaveLength(1);
	});

	it("returns canonical no-match and ambiguous branch associations without guessing", async () => {
		const inspection = available_inspection([
			{
				accounts: [
					{
						active: true,
						git_protocol: "https",
						host: "github.com",
						login: "alice",
						scopes: [],
						type: "authenticated",
					},
				],
				host: "github.com",
			},
		]);
		const read_input = {
			expected_head: "a".repeat(40),
			repository: projected_repository("editor", "main").identity,
			selected_branch: "feature/read",
			selection,
		};
		const no_match_provider = await make_provider({
			inspection,
			read_pull_request: () =>
				Effect.succeed({ type: "no_pull_request", viewer_login: "alice" }),
		});
		const no_match = await Effect.runPromise(no_match_provider.ReadPullRequest!(read_input));

		expect(no_match).toEqual({
			association: { _tag: "none" },
			branch: "feature/read",
			expected_head_commit: "a".repeat(40),
			repository: read_input.repository,
		});

		const ambiguous_provider = await make_provider({
			inspection,
			read_pull_request: () =>
				Effect.succeed({
					candidates: [
						{
							baseRefName: "main",
							headRefName: "feature/read",
							headRefOid: "b".repeat(40),
							headRepository: { name: "editor", owner: { login: "artisan" } },
							id: "PR_node_1",
							isDraft: false,
							isMerged: false,
							number: 1,
							state: "OPEN",
							title: "Read safely",
							url: "https://github.com/artisan/editor/pull/1",
						},
						{
							baseRefName: "main",
							headRefName: "feature/read",
							headRefOid: "c".repeat(40),
							headRepository: { name: "editor", owner: { login: "artisan" } },
							id: "PR_node_2",
							isDraft: true,
							isMerged: true,
							number: 2,
							state: "MERGED",
							title: "Old read",
							url: "https://github.com/artisan/editor/pull/2",
						},
					],
					complete: true,
					type: "ambiguous_pull_requests",
					viewer_login: "alice",
				}),
		});
		const ambiguous = await Effect.runPromise(ambiguous_provider.ReadPullRequest!(read_input));

		expect(ambiguous).toMatchObject({
			association: { _tag: "ambiguous", candidates_truncated: false },
		});
		expect(
			ambiguous.association._tag === "ambiguous" &&
				ambiguous.association.candidates.map((candidate) => candidate.state),
		).toEqual(["open", "merged"]);
	});

	it("maps an exact matched pull request with reviews, threads, checks, and stale-head facts", async () => {
		const inspection = available_inspection([
			{
				accounts: [
					{
						active: true,
						git_protocol: "https",
						host: "github.com",
						login: "alice",
						scopes: [],
						type: "authenticated",
					},
				],
				host: "github.com",
			},
		]);
		const provider = await make_provider({
			inspection,
			read_pull_request: () =>
				Effect.succeed({
					pull_request: matched_pull_request(),
					type: "matched_pull_request",
					viewer_login: "alice",
				}),
		});
		const input = {
			expected_head: "a".repeat(40),
			repository: projected_repository("editor", "main").identity,
			selected_branch: "feature/read",
			selection,
		};
		const current = await Effect.runPromise(provider.ReadPullRequest!(input));

		expect(current).toMatchObject({
			association: {
				_tag: "matched",
				freshness: "current",
				pull_request: {
					checks_total: 3,
					checks_truncated: false,
					mergeability: "mergeable",
					requested_reviewers: [
						{ _tag: "user", login: "bob" },
						{ _tag: "team", organization: "artisan", slug: "maintainers" },
					],
					review_decision: "changes_requested",
					review_threads_total: 101,
					review_threads_truncated: true,
					reviews_total: 2,
					reviews_truncated: false,
				},
			},
		});

		if (current.association._tag !== "matched") {
			throw new Error("Expected a matched pull request");
		}

		expect(current.association.pull_request.reviews).toMatchObject([
			{ author: "carol", state: "changes_requested" },
			{ state: "dismissed" },
		]);
		expect(current.association.pull_request.review_threads[0]).toMatchObject({
			comment_count: 2,
			last_comment_native_id: "comment-2",
			last_updated_at: "2026-07-14T09:30:00Z",
			line: 17,
			outdated: true,
			resolved: false,
			subject: "line",
		});
		expect(current.association.pull_request.checks).toMatchObject([
			{
				annotations: [{ untrusted_message: "untrusted failure" }],
				annotations_truncated: true,
				attempt: 2,
				name: "build",
				required: true,
				state: "failed",
				workflow_name: "CI",
			},
			{ name: "deploy", required: false, state: "running" },
			{ name: "lint", required: true, state: "passed" },
		]);
		expect(provider.Descriptor.capabilities).toEqual(
			expect.arrayContaining([
				{ _tag: "available", capability: "read_reviews" },
				{ _tag: "available", capability: "read_ci" },
			]),
		);

		const stale = await Effect.runPromise(
			provider.ReadPullRequest!({ ...input, expected_head: "f".repeat(40) }),
		);

		expect(stale).toMatchObject({ association: { _tag: "matched", freshness: "stale_head" } });
	});

	it("reads an exact pull-request target directly and preserves current or stale-head freshness", async () => {
		const calls: Array<Parameters<(typeof GitHubCli.Service)["ReadPullRequestTarget"]>[0]> = [];
		const provider = await make_provider({
			inspection: available_inspection([
				{
					accounts: [
						{
							active: true,
							git_protocol: "https",
							host: "github.com",
							login: "alice",
							scopes: [],
							type: "authenticated",
						},
					],
					host: "github.com",
				},
			]),
			read_pull_request_target: (input) => {
				calls.push(input);
				return Effect.succeed({
					pull_request: matched_pull_request(),
					type: "matched_pull_request",
					viewer_login: "alice",
				});
			},
		});
		const input = {
			expected_head: "a".repeat(40),
			pull_request_number: 7,
			pull_request_origin: {
				native_id: "pull-request-7",
				provider_id: "github",
				resource_kind: "pull_request" as const,
			},
			repository: projected_repository("editor", "main").identity,
			selected_branch: "feature/read",
			selection,
		};
		const current = await Effect.runPromise(provider.ReadPullRequestTarget!(input));
		const stale = await Effect.runPromise(
			provider.ReadPullRequestTarget!({ ...input, expected_head: "f".repeat(40) }),
		);

		expect(current).toMatchObject({ association: { _tag: "matched", freshness: "current" } });
		expect(stale).toMatchObject({ association: { _tag: "matched", freshness: "stale_head" } });
		expect(calls).toEqual([
			{
				host: "github.com",
				name: "editor",
				owner: "artisan",
				pull_request_number: 7,
				pull_request_native_id: "pull-request-7",
				selected_branch: "feature/read",
			},
			{
				host: "github.com",
				name: "editor",
				owner: "artisan",
				pull_request_number: 7,
				pull_request_native_id: "pull-request-7",
				selected_branch: "feature/read",
			},
		]);
	});

	it("rejects malformed target input and mismatched target responses before publishing", async () => {
		const input = {
			expected_head: "a".repeat(40),
			pull_request_number: 7,
			pull_request_origin: {
				native_id: "pull-request-7",
				provider_id: "github",
				resource_kind: "pull_request" as const,
			},
			repository: projected_repository("editor", "main").identity,
			selected_branch: "feature/read",
			selection,
		};
		const inspection = available_inspection([
			{
				accounts: [
					{
						active: true,
						git_protocol: "https",
						host: "github.com",
						login: "alice",
						scopes: [],
						type: "authenticated",
					},
				],
				host: "github.com",
			},
		]);
		const malformed = await make_provider({ inspection });

		await expect(
			Effect.runPromise(
				malformed.ReadPullRequestTarget!({ ...input, pull_request_number: 0 } as never),
			),
		).rejects.toMatchObject({ operation: "read_pull_request_target", reason: "invalid_input" });
		await expect(
			Effect.runPromise(
				malformed.ReadPullRequestTarget!({ ...input, unexpected: true } as never),
			),
		).rejects.toMatchObject({ operation: "read_pull_request_target", reason: "invalid_input" });

		const inactive = await make_provider({
			inspection: available_inspection([
				{
					accounts: [
						{
							active: true,
							git_protocol: "https",
							host: "github.com",
							login: "bob",
							scopes: [],
							type: "authenticated",
						},
					],
					host: "github.com",
				},
			]),
		});

		await expect(
			Effect.runPromise(inactive.ReadPullRequestTarget!(input)),
		).rejects.toMatchObject({
			operation: "read_pull_request_target",
			reason: "account_not_active",
		});

		for (const pull_request of [
			{ ...matched_pull_request(), number: 8 },
			{ ...matched_pull_request(), id: "recreated-pull-request-7" },
			{ ...matched_pull_request(), headRefName: "other-branch" },
			{
				...matched_pull_request(),
				headRepository: { name: "other-repository", owner: { login: "artisan" } },
			},
		] as const) {
			const provider = await make_provider({
				inspection,
				read_pull_request_target: () =>
					Effect.succeed({
						pull_request,
						type: "matched_pull_request",
						viewer_login: "alice",
					}),
			});

			await expect(
				Effect.runPromise(provider.ReadPullRequestTarget!(input)),
			).rejects.toMatchObject({
				operation: "read_pull_request_target",
				reason: "invalid_response",
			});
		}

		const mismatched_viewer = await make_provider({
			inspection,
			read_pull_request_target: () =>
				Effect.succeed({
					pull_request: matched_pull_request(),
					type: "matched_pull_request",
					viewer_login: "bob",
				}),
		});

		await expect(
			Effect.runPromise(mismatched_viewer.ReadPullRequestTarget!(input)),
		).rejects.toMatchObject({
			operation: "read_pull_request_target",
			reason: "account_not_active",
		});
	});

	it("rejects inactive or mismatched accounts before publishing a pull-request read", async () => {
		const read_input = {
			expected_head: "a".repeat(40),
			repository: projected_repository("editor", "main").identity,
			selected_branch: "feature/read",
			selection,
		};
		const inactive = await make_provider({
			inspection: available_inspection([
				{
					accounts: [
						{
							active: true,
							git_protocol: "https",
							host: "github.com",
							login: "bob",
							scopes: [],
							type: "authenticated",
						},
					],
					host: "github.com",
				},
			]),
		});

		await expect(
			Effect.runPromise(inactive.ReadPullRequest!(read_input)),
		).rejects.toMatchObject({
			operation: "read_pull_request",
			reason: "account_not_active",
		});

		const mismatched = await make_provider({
			inspection: available_inspection([
				{
					accounts: [
						{
							active: true,
							git_protocol: "https",
							host: "github.com",
							login: "alice",
							scopes: [],
							type: "authenticated",
						},
					],
					host: "github.com",
				},
			]),
			read_pull_request: () =>
				Effect.succeed({ type: "no_pull_request", viewer_login: "bob" }),
		});

		await expect(
			Effect.runPromise(mismatched.ReadPullRequest!(read_input)),
		).rejects.toMatchObject({ operation: "read_pull_request", reason: "account_not_active" });
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
				clone_url: "https://github.com/artisan/private-repo.git",
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

	it("prepares a clone from a fresh repository observation under the selected account", async () => {
		const provider = await make_provider({
			inspection: available_inspection([
				{
					accounts: [
						{
							active: true,
							git_protocol: "https",
							host: "github.com",
							login: "alice",
							scopes: [],
							type: "authenticated",
						},
					],
					host: "github.com",
				},
			]),
			inspect_repository: () =>
				Effect.succeed({ repository: repository("editor", "main"), viewer_login: "alice" }),
		});
		const preparation = await Effect.runPromise(
			provider.PrepareClone({ repository: projected_repository("editor"), selection }),
		);

		expect(preparation).toEqual({
			repository: projected_repository("editor", "main"),
			selection,
		});
		expect(
			provider.Descriptor.capabilities.find(
				(capability) => capability.capability === "clone_repository",
			),
		).toEqual({ _tag: "available", capability: "clone_repository" });
	});

	it("attributes malformed preparation responses to clone preparation", async () => {
		const malformed = {
			...repository("editor", "main"),
			name_with_owner: "another/editor",
		};
		const provider = await make_provider({
			inspection: available_inspection([
				{
					accounts: [
						{
							active: true,
							git_protocol: "https",
							host: "github.com",
							login: "alice",
							scopes: [],
							type: "authenticated",
						},
					],
					host: "github.com",
				},
			]),
			inspect_repository: () =>
				Effect.succeed({ repository: malformed, viewer_login: "alice" }),
		});

		await expect(
			Effect.runPromise(
				provider.PrepareClone({ repository: projected_repository("editor"), selection }),
			),
		).rejects.toMatchObject({ operation: "prepare_clone", reason: "invalid_response" });
	});

	it("rejects a stale provider-native repository identity before cloning", async () => {
		const provider = await make_provider({
			inspection: available_inspection([
				{
					accounts: [
						{
							active: true,
							git_protocol: "https",
							host: "github.com",
							login: "alice",
							scopes: [],
							type: "authenticated",
						},
					],
					host: "github.com",
				},
			]),
			inspect_repository: () =>
				Effect.succeed({ repository: repository("editor"), viewer_login: "alice" }),
		});
		const stale = {
			...projected_repository("editor"),
			origin: {
				...projected_repository("editor").origin,
				native_id: "replaced-native-id",
			},
		};

		await expect(
			Effect.runPromise(provider.PrepareClone({ repository: stale, selection })),
		).rejects.toMatchObject({ operation: "prepare_clone", reason: "stale_repository" });
	});

	it("reports a missing pinned Git executable separately from a missing GitHub CLI", async () => {
		const provider = await make_provider({
			clone: () =>
				Effect.fail(
					new GitHubCliError({
						operation: "clone_repository",
						reason: "git_dependency_missing",
						retryable: false,
					}),
				),
			inspection: available_inspection([
				{
					accounts: [
						{
							active: true,
							git_protocol: "https",
							host: "github.com",
							login: "alice",
							scopes: [],
							type: "authenticated",
						},
					],
					host: "github.com",
				},
			]),
			inspect_repository: () =>
				Effect.succeed({
					repository: repository("editor", "main"),
					viewer_login: "alice",
				}),
		});
		const error = await Effect.runPromise(
			provider
				.Clone({
					destination: clone_destination(),
					preparation: {
						repository: projected_repository("editor", "main"),
						selection,
					},
				})
				.pipe(Effect.flip),
		);

		expect(error).toMatchObject({
			operation: "clone_repository",
			reason: "git_missing",
			retryable: false,
		});
	});

	it("rejects caller-supplied clone templates at the public execution boundary", async () => {
		await expect(
			Effect.runPromise(
				Schema.decodeUnknownEffect(GitProviderCloneExecution, {
					onExcessProperty: "error",
				})({
					destination: clone_destination(),
					preparation: {
						repository: projected_repository("editor", "main"),
						selection,
					},
					template_path: "C:\\Artisan\\attacker-template",
				}),
			),
		).rejects.toBeDefined();
	});

	it("clones once and rechecks repository ownership after execution", async () => {
		const clone_inputs: Array<Parameters<(typeof GitHubCli.Service)["CloneRepository"]>[0]> =
			[];
		let inspection_count = 0;
		let verification_count = 0;
		const provider = await make_provider({
			clone: (input) => {
				clone_inputs.push(input);

				return Effect.succeed({
					VerifyCheckout: Effect.sync(() => {
						verification_count += 1;
					}),
					canonical_root: input.destination.canonical_root,
					output_complete: true,
				});
			},
			inspection: available_inspection([
				{
					accounts: [
						{
							active: true,
							git_protocol: "https",
							host: "github.com",
							login: "alice",
							scopes: [],
							type: "authenticated",
						},
					],
					host: "github.com",
				},
			]),
			inspect_repository: () => {
				inspection_count += 1;

				return Effect.succeed({
					repository: repository("editor", "main"),
					viewer_login: "alice",
				});
			},
		});
		const result = await Effect.runPromise(
			provider.Clone({
				destination: clone_destination(),
				preparation: {
					repository: projected_repository("editor", "main"),
					selection,
				},
			}),
		);

		expect(result).toEqual({
			canonical_root: "C:\\Projects\\editor",
			output_complete: true,
			repository: projected_repository("editor", "main"),
			type: "cloned",
		});
		expect(inspection_count).toBe(2);
		expect(verification_count).toBe(1);
		expect(clone_inputs).toEqual([
			{
				account_login: "alice",
				destination: clone_destination(),
				host: "github.com",
				name: "editor",
				owner: "artisan",
			},
		]);
	});

	it("revalidates the checkout after the remote postcheck", async () => {
		const postcheck_started = await Effect.runPromise(Deferred.make<void>());
		const release_postcheck = await Effect.runPromise(Deferred.make<void>());
		const checkout_replaced = await Effect.runPromise(Ref.make(false));
		let inspection_count = 0;
		const provider = await make_provider({
			clone: () =>
				Effect.succeed({
					VerifyCheckout: Ref.get(checkout_replaced).pipe(
						Effect.flatMap((replaced) =>
							replaced
								? Effect.fail(
										new GitHubCliError({
											operation: "clone_repository",
											reason: "outcome_unknown",
											retryable: false,
										}),
									)
								: Effect.void,
						),
					),
					canonical_root: "C:\\Projects\\editor",
					output_complete: true,
				}),
			inspection: available_inspection([
				{
					accounts: [
						{
							active: true,
							git_protocol: "https",
							host: "github.com",
							login: "alice",
							scopes: [],
							type: "authenticated",
						},
					],
					host: "github.com",
				},
			]),
			inspect_repository: () => {
				inspection_count += 1;

				const result = {
					repository: repository("editor", "main"),
					viewer_login: "alice",
				};

				return inspection_count === 1
					? Effect.succeed(result)
					: Deferred.succeed(postcheck_started, undefined).pipe(
							Effect.andThen(Deferred.await(release_postcheck)),
							Effect.as(result),
						);
			},
		});
		const exit = await Effect.runPromise(
			Effect.gen(function* () {
				const fiber = yield* Effect.forkChild(
					provider.Clone({
						destination: clone_destination(),
						preparation: {
							repository: projected_repository("editor", "main"),
							selection,
						},
					}),
				);

				yield* Deferred.await(postcheck_started);
				yield* Ref.set(checkout_replaced, true);
				yield* Deferred.succeed(release_postcheck, undefined);

				return yield* Fiber.await(fiber);
			}),
		);

		expect(Exit.isFailure(exit) ? Cause.squash(exit.cause) : undefined).toMatchObject({
			operation: "clone_repository",
			reason: "outcome_unknown",
			retryable: false,
		});
		expect(inspection_count).toBe(2);
	});

	it("quarantines a completed clone when post-execution identity cannot be proven", async () => {
		let clone_count = 0;
		let inspection_count = 0;
		const provider = await make_provider({
			clone: () => {
				clone_count += 1;

				return Effect.succeed({
					VerifyCheckout: Effect.void,
					canonical_root: "C:\\Projects\\editor",
					output_complete: true,
				});
			},
			inspection: available_inspection([
				{
					accounts: [
						{
							active: true,
							git_protocol: "https",
							host: "github.com",
							login: "alice",
							scopes: [],
							type: "authenticated",
						},
					],
					host: "github.com",
				},
			]),
			inspect_repository: () => {
				inspection_count += 1;

				return inspection_count === 1
					? Effect.succeed({
							repository: repository("editor", "main"),
							viewer_login: "alice",
						})
					: Effect.fail(
							new GitHubCliError({
								operation: "inspect_repository",
								reason: "network_unavailable",
								retryable: true,
							}),
						);
			},
		});
		const error = await Effect.runPromise(
			provider
				.Clone({
					destination: clone_destination(),
					preparation: {
						repository: projected_repository("editor", "main"),
						selection,
					},
				})
				.pipe(Effect.flip),
		);

		expect(error).toMatchObject({
			operation: "clone_repository",
			reason: "outcome_unknown",
			retryable: false,
		});
		expect(clone_count).toBe(1);
	});

	it("converts cancellation during post-clone verification into an unknown outcome", async () => {
		const postcheck_started = await Effect.runPromise(Deferred.make<void>());
		let inspection_count = 0;
		const provider = await make_provider({
			clone: () =>
				Effect.succeed({
					VerifyCheckout: Effect.void,
					canonical_root: "C:\\Projects\\editor",
					output_complete: true,
				}),
			inspection: available_inspection([
				{
					accounts: [
						{
							active: true,
							git_protocol: "https",
							host: "github.com",
							login: "alice",
							scopes: [],
							type: "authenticated",
						},
					],
					host: "github.com",
				},
			]),
			inspect_repository: () => {
				inspection_count += 1;

				return inspection_count === 1
					? Effect.succeed({
							repository: repository("editor", "main"),
							viewer_login: "alice",
						})
					: Deferred.succeed(postcheck_started, undefined).pipe(
							Effect.andThen(Effect.never),
						);
			},
		});
		const error = await Effect.runPromise(
			Effect.gen(function* () {
				const fiber = yield* Effect.forkChild(
					provider.Clone({
						destination: clone_destination(),
						preparation: {
							repository: projected_repository("editor", "main"),
							selection,
						},
					}),
				);

				yield* Deferred.await(postcheck_started);
				yield* Fiber.interrupt(fiber);
				const exit = yield* Fiber.await(fiber);

				return Exit.isFailure(exit)
					? Cause.squash(exit.cause)
					: yield* Effect.die("Cancellation must fail provider verification");
			}),
		);

		expect(error).toMatchObject({
			operation: "clone_repository",
			reason: "outcome_unknown",
			retryable: false,
		});
	});
});

describe("GitHubProvider check failure detail", () => {
	it("publishes only an exact selected-account check detail and rejects changed identity", async () => {
		const input = {
			check_origin: {
				native_id: "check-1",
				provider_id: "github",
				resource_kind: "check_run" as const,
			},
			expected_head: "a".repeat(40),
			pull_request_number: 7,
			pull_request_origin: {
				native_id: "pull-request-7",
				provider_id: "github",
				resource_kind: "pull_request" as const,
			},
			repository: projected_repository("editor", "main").identity,
			selected_branch: "feature/read",
			selection,
		};
		const inspection = available_inspection([
			{
				accounts: [
					{
						active: true,
						git_protocol: "https",
						host: "github.com",
						login: "alice",
						scopes: [],
						type: "authenticated",
					},
				],
				host: "github.com",
			},
		]);
		const detail = {
			attempt: 2,
			check_native_id: "check-1",
			head_commit: "a".repeat(40),
			log: {
				_tag: "available" as const,
				observed_bytes: 12,
				truncated: false,
				untrusted_excerpt: "failed test",
			},
			name: "build",
			output: {
				summary: {
					_tag: "available" as const,
					truncated: false,
					untrusted_text: "summary",
				},
				text: { _tag: "unavailable" as const },
				title: "failure",
			},
			viewer_login: "Alice",
			workflow_native_id: "run-1",
		};
		const provider = await make_provider({
			inspection,
			read_check_failure_detail: () => Effect.succeed(detail),
		});

		await expect(
			Effect.runPromise(provider.ReadCheckFailureDetail!(input)),
		).resolves.toMatchObject({
			attempt: 2,
			check_origin: input.check_origin,
			head_commit: input.expected_head,
			workflow_origin: { native_id: "run-1", resource_kind: "workflow_run" },
		});

		const changed = await make_provider({
			inspection,
			read_check_failure_detail: () =>
				Effect.succeed({ ...detail, check_native_id: "replaced-check" }),
		});
		await expect(
			Effect.runPromise(changed.ReadCheckFailureDetail!(input)),
		).rejects.toMatchObject({
			operation: "read_check_failure_detail",
			reason: "invalid_response",
		});

		const mismatched_viewer = await make_provider({
			inspection,
			read_check_failure_detail: () => Effect.succeed({ ...detail, viewer_login: "bob" }),
		});
		await expect(
			Effect.runPromise(mismatched_viewer.ReadCheckFailureDetail!(input)),
		).rejects.toMatchObject({
			operation: "read_check_failure_detail",
			reason: "account_not_active",
		});
	});
});
