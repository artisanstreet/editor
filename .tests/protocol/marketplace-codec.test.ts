import { Effect, Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeMarketplaceEventBatch,
	DecodeMarketplaceSource,
	DecodeMcpCapability,
	DecodeMcpConnectPreview,
	DecodeMcpConnectionApproval,
	DecodeMcpTransport,
	DecodeRoutine,
	DecodeRoutineInstallApproval,
	McpCapability,
	Routine,
} from "../../modules/protocol/src/marketplace";

const timestamp = "2026-07-17T12:00:00.000Z";
const source = {
	display_name: "Artisan catalog",
	kind: "catalog",
	locator: "artisan.catalog.release-notes",
	revision: "a".repeat(40),
} as const;
const routine_identity = { source, version: "1.2.0" } as const;
const mcp_identity = {
	source: {
		display_name: "GitHub MCP",
		kind: "plugin",
		locator: "github.mcp",
	},
	version: "2.3.1",
} as const;
const trust = { level: "verified", reasons: ["Publisher identity is verified."] } as const;
const scope = { kind: "project", project_id: "project_1" } as const;
const permission = {
	kind: "filesystem_write",
	label: "Write release notes",
	required: true,
} as const;
const secret = { purpose: "GitHub API access", secret_id: "secret.github.token" } as const;
const routine = {
	commands: [
		{
			command_id: "release_notes.create",
			description: "Creates a reviewed release-note draft.",
			label: "Create release notes",
		},
	],
	compatibility: ["codex", "claude"],
	display_name: "Release notes",
	files: [
		{
			path: ".artisan/routines/release-notes.md",
			purpose: "Routine instructions",
			write_mode: "create",
		},
	],
	lifecycle: "enabled",
	permissions: [permission],
	scope,
	summary: {
		description: "Creates repository-aware release notes.",
		display_name: "Release notes",
		identity: routine_identity,
		routine_id: "routine.release_notes",
	},
	sync: [
		{
			drift: "none",
			engine: "codex",
			identity: routine_identity,
			status: "synced",
			updated_at: timestamp,
		},
	],
	trust,
	updated_at: timestamp,
} as const;
const stdio_transport = {
	arguments: [
		{ kind: "positional", value: "@modelcontextprotocol/server-filesystem" },
		{ kind: "positional", value: "./notes" },
		{ kind: "option", name: "verbose" },
		{ kind: "option_value", name: "log-level", value: "debug" },
	],
	command: "npx",
	environment: [{ name: "GITHUB_TOKEN", secret }],
	kind: "stdio",
} as const;
const mcp = {
	auth: { kind: "bearer", secret },
	capability_id: "mcp.github",
	compatibility: ["codex"],
	display_name: "GitHub",
	health: "healthy",
	identity: mcp_identity,
	instructions: {
		content: "Use this MCP for repository metadata.",
		content_hash: "a".repeat(64),
	},
	lifecycle: "connected",
	permissions: [{ kind: "network_connect", label: "Connect to GitHub", required: true }],
	prompts: [{ name: "review_pull_request" }],
	resources: [{ name: "repository_metadata" }],
	scope,
	sync: [
		{
			drift: "none",
			engine: "codex",
			identity: mcp_identity,
			status: "synced",
			updated_at: timestamp,
		},
	],
	tool_policy: [
		{
			approval_policy: "required",
			label: "Read pull request",
			status: "allowed",
			tool_name: "get_pull_request",
		},
	],
	tools: [{ description: "Reads one pull request.", name: "get_pull_request" }],
	transport: stdio_transport,
	trust,
	updated_at: timestamp,
} as const;
const install_approval = {
	approval_id: "approval.routine.release_notes",
	decision: "pending",
	preview: {
		compatibility: routine.compatibility,
		files: routine.files,
		identity: routine_identity,
		permissions: routine.permissions,
		rollback: {
			actions: ["Remove created routine files."],
			available: true,
			identity: routine_identity,
			rollback_id: "rollback_1",
		},
		scope,
		trust,
	},
	routine_id: routine.summary.routine_id,
	updated_at: timestamp,
} as const;

describe("Marketplace codec", () => {
	it("roundtrips canonical Routine, MCP, approval, and source-safe event projections", async () => {
		const decoded_routine = await Effect.runPromise(DecodeRoutine(routine));
		const decoded_mcp = await Effect.runPromise(DecodeMcpCapability(mcp));

		expect(Schema.encodeSync(Routine)(decoded_routine)).toEqual(routine);
		expect(Schema.encodeSync(McpCapability)(decoded_mcp)).toEqual(mcp);
		await expect(
			Effect.runPromise(DecodeRoutineInstallApproval(install_approval)),
		).resolves.toEqual(install_approval);
		await expect(
			Effect.runPromise(
				DecodeMarketplaceEventBatch({
					events: [
						{
							entry_id: "routine.release_notes",
							entry_kind: "routine",
							lifecycle: "enabled",
							type: "marketplace.lifecycle.updated",
							updated_at: timestamp,
						},
						{
							approval_required: true,
							entry_id: "mcp.github",
							entry_kind: "mcp",
							invocation_id: "invoke_1",
							state: "approval_required",
							type: "marketplace.invocation.updated",
						},
					],
					schema_version: 1,
				}),
			),
		).resolves.toHaveProperty("events", expect.any(Array));
	});

	it("accepts realistic canonical source and stdio variants", async () => {
		for (const value of [
			{ display_name: "Local routine", kind: "local", locator: "routines/release-notes" },
			{
				display_name: "Git routine",
				kind: "git",
				locator: "https://github.com/artisan/routines.git",
				revision: "b".repeat(40),
			},
			{ display_name: "Package routine", kind: "package", locator: "@artisan/release-notes" },
			{ display_name: "Catalog routine", kind: "catalog", locator: "artisan.release-notes" },
			{ display_name: "Provider import", kind: "provider", locator: "codex.release-notes" },
			{ display_name: "Plugin bundle", kind: "plugin", locator: "github.mcp" },
		] as const) {
			await expect(Effect.runPromise(DecodeMarketplaceSource(value))).resolves.toEqual(value);
		}

		await expect(Effect.runPromise(DecodeMcpTransport(stdio_transport))).resolves.toEqual(
			stdio_transport,
		);
	});

	it.each([
		"https://mcp.example.com/v1",
		"https://8.8.8.8/mcp",
		"https://[2606:4700:4700::1111]/mcp",
	])(
		"accepts public HTTPS MCP endpoint %s with a remote, self-contained preview",
		async (endpoint) => {
			const transport = { endpoint, kind: "streamable_http" } as const;
			const preview = {
				capability: {
					...mcp,
					auth: {
						kind: "oauth",
						scopes: ["repo:read"],
						status: "authorization_required",
					},
					transport,
				},
				network: "remote",
			} as const;

			await expect(
				Effect.runPromise(DecodeMcpCapability({ ...mcp, transport })),
			).resolves.toMatchObject({ transport });
			await expect(Effect.runPromise(DecodeMcpConnectPreview(preview))).resolves.toEqual(
				preview,
			);
		},
	);

	it.each([
		"http://mcp.example.com/v1",
		"https://localhost/mcp",
		"https://mcp.localhost/v1",
		"https://127.0.0.1/mcp",
		"https://10.0.0.1/mcp",
		"https://169.254.1.1/mcp",
		"https://224.0.0.1/mcp",
		"https://0.0.0.0/mcp",
		"https://100.64.0.1/mcp",
		"https://192.0.2.1/mcp",
		"https://[::1]/mcp",
		"https://[::]/mcp",
		"https://[fe80::1]/mcp",
		"https://[fc00::1]/mcp",
		"https://[ff02::1]/mcp",
		"https://[::ffff:127.0.0.1]/mcp",
		"https://user:password@mcp.example.com/v1",
		"https://user@mcp.example.com/v1",
		"https:\\mcp.example.com\\@127.0.0.1/mcp",
		"https://mcp.example.com\\@127.0.0.1/mcp",
		" https://mcp.example.com/v1",
		"https://mcp.example.com/v1 ",
		"https://mcp.example.com/v1\n",
		"https://mcp.example.com/v1?token=private",
	])("rejects unsafe MCP endpoint %s at the codec boundary", async (endpoint) => {
		await expect(
			Effect.runPromise(
				DecodeMcpCapability({
					...mcp,
					transport: { endpoint, kind: "streamable_http" },
				}),
			),
		).rejects.toBeDefined();
	});

	it("rejects source URL normalization, whitespace, and secret-like revisions", async () => {
		for (const locator of [
			"https://user:password@github.com/artisan/routines.git",
			"https://user@github.com/artisan/routines.git",
			"https:\\github.com\\artisan\\routines.git",
			"https://github.com\\@evil.example/routines.git",
			" https://github.com/artisan/routines.git",
			"https://github.com/artisan/routines.git ",
			"https://github.com/artisan/routines.git\n",
			"https://github.com/artisan/routines.git?token=private",
		]) {
			await expect(
				Effect.runPromise(
					DecodeMarketplaceSource({ display_name: "Git routine", kind: "git", locator }),
				),
			).rejects.toBeDefined();
		}

		for (const revision of [
			"TOKEN=private",
			"--auth private",
			"ghp_private",
			` ${"a".repeat(40)}`,
			`${"a".repeat(40)}\n`,
			"a".repeat(39),
		]) {
			await expect(
				Effect.runPromise(DecodeMarketplaceSource({ ...source, revision })),
			).rejects.toBeDefined();
		}

		for (const display_name of [" Git routine", "Git routine ", "Git\nRoutine"]) {
			await expect(
				Effect.runPromise(DecodeMarketplaceSource({ ...source, display_name })),
			).rejects.toBeDefined();
		}
	});

	it("keeps stdio syntax canonical and all sensitive injection reference-only", async () => {
		for (const transport of [
			{ ...stdio_transport, command: "npx --token private" },
			{ ...stdio_transport, command: " npx" },
			{ ...stdio_transport, command: "npx\n" },
			{ ...stdio_transport, command: "https:\\example.com\\server" },
			{ ...stdio_transport, working_directory: " notes" },
			{ ...stdio_transport, working_directory: "notes\\private" },
			{ ...stdio_transport, working_directory: "notes\nprivate" },
			{ ...stdio_transport, arguments: ["--api-key=private"] },
			{
				...stdio_transport,
				arguments: [{ kind: "positional", value: "GITHUB_TOKEN=private" }],
			},
			{
				...stdio_transport,
				arguments: [{ kind: "positional", value: "1FOO=private" }],
			},
			{
				...stdio_transport,
				arguments: [{ kind: "positional", value: "FOO+=private" }],
			},
			{
				...stdio_transport,
				arguments: [{ kind: "positional", value: "A.B=private" }],
			},
			{
				...stdio_transport,
				arguments: [{ kind: "positional", value: "user:password@example.com" }],
			},
			{
				...stdio_transport,
				arguments: [{ kind: "positional", value: "https://user:password@example.com/mcp" }],
			},
			{
				...stdio_transport,
				arguments: [{ kind: "positional", value: "https://example.com/mcp?token=private" }],
			},
			{
				...stdio_transport,
				arguments: [
					{ kind: "option_value", name: "header", value: "Authorization:Bearer=private" },
				],
			},
			{
				...stdio_transport,
				arguments: [
					{ kind: "option_value", name: "header", value: "Bearer private-token" },
				],
			},
			{
				...stdio_transport,
				environment: [{ name: "GITHUB_TOKEN", value: "private" }],
			},
			{
				...stdio_transport,
				environment: [
					{ name: "GITHUB_TOKEN", secret: { ...secret, secret_id: "token=private" } },
				],
			},
			{
				...stdio_transport,
				environment: [
					stdio_transport.environment[0],
					{ ...stdio_transport.environment[0], name: "github_token" },
				],
			},
		]) {
			await expect(Effect.runPromise(DecodeMcpTransport(transport))).rejects.toBeDefined();
		}

		for (const name of [
			"api-key",
			"apikey",
			"auth",
			"authentication",
			"authorization-header",
			"basic-credential",
			"bearer-token",
			"credential-file",
			"env",
			"env-file",
			"github-token",
			"key",
			"oauth",
			"password",
			"client-secret",
			"username",
		]) {
			await expect(
				Effect.runPromise(
					DecodeMcpTransport({
						...stdio_transport,
						arguments: [{ kind: "option", name }],
					}),
				),
			).rejects.toBeDefined();
		}

		for (const value of ["C:/Project Files/notes", "føø/notes/*.md", "-1", "-1e-6", "-.5"]) {
			await expect(
				Effect.runPromise(
					DecodeMcpTransport({
						...stdio_transport,
						arguments: [{ kind: "positional", value }],
					}),
				),
			).resolves.toBeDefined();
		}
	});

	it("requires canonical MCP machine names and total unique capability policies", async () => {
		for (const value of [
			{ ...mcp, tools: [{ name: "Read Pull Request" }] },
			{ ...mcp, tools: [{ name: "get/pull_request" }] },
			{ ...mcp, resources: [{ name: " repository_metadata" }] },
			{ ...mcp, prompts: [{ name: "review__pull_request" }] },
			{ ...mcp, tool_policy: [{ ...mcp.tool_policy[0], tool_name: "GetPullRequest" }] },
			{ ...mcp, tools: [...mcp.tools, mcp.tools[0]] },
			{ ...mcp, resources: [...mcp.resources, mcp.resources[0]] },
			{ ...mcp, prompts: [...mcp.prompts, mcp.prompts[0]] },
			{ ...mcp, tool_policy: [...mcp.tool_policy, mcp.tool_policy[0]] },
			{ ...mcp, tool_policy: [] },
			{
				...mcp,
				tool_policy: [{ ...mcp.tool_policy[0], tool_name: "delete_repository" }],
			},
		]) {
			await expect(Effect.runPromise(DecodeMcpCapability(value))).rejects.toBeDefined();
		}
	});

	it("binds approval to complete capabilities and truthful network disclosure", async () => {
		const http_transport = {
			endpoint: "https://mcp.example.com/v1",
			kind: "streamable_http",
		} as const;
		const preview = {
			capability: { ...mcp, auth: { kind: "none" }, transport: http_transport },
			network: "remote",
		} as const;
		const approval = {
			approval_id: "approval.mcp.github",
			capability_id: mcp.capability_id,
			decision: "pending",
			preview,
			updated_at: timestamp,
		} as const;

		await expect(Effect.runPromise(DecodeMcpConnectPreview(preview))).resolves.toEqual(preview);
		await expect(Effect.runPromise(DecodeMcpConnectionApproval(approval))).resolves.toEqual(
			approval,
		);
		await expect(
			Effect.runPromise(DecodeMcpConnectPreview({ ...preview, network: "none" })),
		).rejects.toBeDefined();
		await expect(
			Effect.runPromise(
				DecodeMcpConnectionApproval({ ...approval, capability_id: "mcp.other" }),
			),
		).rejects.toBeDefined();

		const resource_only = {
			...mcp,
			capability_id: "mcp.resource_only",
			permissions: [],
			tool_policy: [],
			tools: [],
		} as const;
		const resource_preview = { capability: resource_only, network: "none" } as const;

		await expect(Effect.runPromise(DecodeMcpConnectPreview(resource_preview))).resolves.toEqual(
			resource_preview,
		);
		await expect(
			Effect.runPromise(
				DecodeMcpConnectPreview({
					capability: mcp,
					network: "none",
				}),
			),
		).rejects.toBeDefined();
		await expect(
			Effect.runPromise(
				DecodeMcpConnectPreview({
					capability: { ...resource_only, permissions: mcp.permissions },
					network: "none",
				}),
			),
		).rejects.toBeDefined();
	});

	it("classifies explicit stdio endpoints before connection approval", async () => {
		const remote_transport = {
			...stdio_transport,
			arguments: [
				...stdio_transport.arguments,
				{ kind: "positional", value: "https://mcp.example.com/v1" },
			],
		} as const;
		const localhost_transport = {
			...stdio_transport,
			arguments: [
				...stdio_transport.arguments,
				{ kind: "positional", value: "http://127.0.0.1:3000/mcp" },
			],
		} as const;
		const remote_capability = { ...mcp, transport: remote_transport } as const;
		const localhost_capability = { ...mcp, transport: localhost_transport } as const;

		await expect(
			Effect.runPromise(DecodeMcpCapability({ ...remote_capability, permissions: [] })),
		).rejects.toBeDefined();
		await expect(
			Effect.runPromise(
				DecodeMcpConnectPreview({ capability: remote_capability, network: "remote" }),
			),
		).resolves.toBeDefined();
		await expect(
			Effect.runPromise(
				DecodeMcpConnectPreview({ capability: remote_capability, network: "localhost" }),
			),
		).rejects.toBeDefined();
		await expect(
			Effect.runPromise(
				DecodeMcpConnectPreview({ capability: localhost_capability, network: "localhost" }),
			),
		).resolves.toBeDefined();
		await expect(
			Effect.runPromise(
				DecodeMcpConnectPreview({ capability: localhost_capability, network: "none" }),
			),
		).rejects.toBeDefined();
		await expect(
			Effect.runPromise(
				DecodeMcpConnectPreview({ capability: localhost_capability, network: "remote" }),
			),
		).rejects.toBeDefined();
	});

	it("rejects duplicate per-engine sync rows and source-version drift", async () => {
		for (const value of [
			{ ...routine, compatibility: ["codex", "codex"] },
			{ ...routine, sync: [...routine.sync, routine.sync[0]] },
			{
				...routine,
				sync: [
					{
						...routine.sync[0],
						identity: { ...routine_identity, version: "1.3.0" },
					},
				],
			},
		]) {
			await expect(Effect.runPromise(DecodeRoutine(value))).rejects.toBeDefined();
		}

		for (const value of [
			{ ...mcp, compatibility: ["codex", "codex"] },
			{ ...mcp, sync: [...mcp.sync, mcp.sync[0]] },
			{
				...mcp,
				sync: [
					{
						...mcp.sync[0],
						identity: { ...mcp_identity, version: "2.4.0" },
					},
				],
			},
		]) {
			await expect(Effect.runPromise(DecodeMcpCapability(value))).rejects.toBeDefined();
		}
	});

	it("rejects malformed versions, rollback identity drift, and excess fields through strict decoders", async () => {
		for (const version of ["1", "01.2.3", `1.0.0+${"a".repeat(129)}`]) {
			await expect(
				Effect.runPromise(
					DecodeMcpCapability({
						...mcp,
						identity: { ...mcp.identity, version },
					}),
				),
			).rejects.toBeDefined();
		}

		await expect(
			Effect.runPromise(
				DecodeRoutineInstallApproval({
					...install_approval,
					preview: {
						...install_approval.preview,
						rollback: {
							...install_approval.preview.rollback,
							identity: { ...routine_identity, version: "1.1.0" },
						},
					},
				}),
			),
		).rejects.toBeDefined();
		await expect(
			Effect.runPromise(DecodeRoutine({ ...routine, provider_config: { private: true } })),
		).rejects.toBeDefined();
		await expect(
			Effect.runPromise(
				DecodeMarketplaceEventBatch({
					events: [
						{
							approval_required: false,
							arguments: { private: true },
							entry_id: "mcp.github",
							entry_kind: "mcp",
							invocation_id: "invoke_1",
							state: "completed",
							type: "marketplace.invocation.updated",
						},
					],
					schema_version: 1,
				}),
			),
		).rejects.toBeDefined();
	});
});
