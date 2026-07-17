import { Schema } from "effect";

import ipaddr from "ipaddr.js";

import { Identifier, IsoDateTime } from "./common";

const text_encoder = new TextEncoder();

/** Defines the largest user-visible Marketplace text field. */
export const marketplace_visible_text_maximum_characters = 2_048;

/** Defines the largest progressive-disclosure instruction payload. */
export const marketplace_instructions_maximum_bytes = 65_536;

/** Defines the largest collection carried by one Marketplace projection. */
export const marketplace_collection_maximum_items = 128;

const bounded_text = (maximum_characters: number, description: string) =>
	Schema.String.check(
		Schema.isMinLength(1),
		Schema.isMaxLength(maximum_characters),
		Schema.makeFilter<string>((value) =>
			value !== value.trim() || /\p{Cc}|\p{Cf}/u.test(value)
				? `Expected ${description} without surrounding whitespace or hidden control characters`
				: undefined,
		),
	);

const bounded_byte_text = (maximum_bytes: number, description: string) =>
	Schema.String.check(
		Schema.makeFilter<string>((value) =>
			text_encoder.encode(value).byteLength <= maximum_bytes
				? undefined
				: `Expected ${description} within ${maximum_bytes} UTF-8 bytes`,
		),
	);

const BoundedIdentifier = Identifier.check(
	Schema.isMaxLength(256),
	Schema.makeFilter<string>((value) =>
		text_encoder.encode(value).byteLength <= 256
			? undefined
			: "Expected an identifier within 256 UTF-8 bytes",
	),
);

const VisibleName = bounded_text(256, "a visible name");
const VisibleSummary = bounded_text(
	marketplace_visible_text_maximum_characters,
	"a visible summary",
);
const RelativePath = Schema.String.check(
	Schema.isMinLength(1),
	Schema.isMaxLength(1_024),
	Schema.makeFilter<string>((value) =>
		value !== value.trim() ||
		/\p{Cc}|\p{Cf}/u.test(value) ||
		value.startsWith("/") ||
		value.includes("\\") ||
		/^[A-Za-z]:/.test(value) ||
		value.split("/").some((segment) => segment === "..")
			? "Expected a canonical relative path without control whitespace, backslashes, or parent traversal"
			: undefined,
	),
);
const BoundedCollection = <S extends Schema.Top>(schema: S) =>
	Schema.Array(schema).check(Schema.isMaxLength(marketplace_collection_maximum_items));

const credential_free_https_url = (description: string, require_public_host: boolean) =>
	Schema.String.check(
		Schema.isMinLength(1),
		Schema.isMaxLength(2_048),
		Schema.makeFilter<string>((value) => {
			if (
				value !== value.trim() ||
				/\p{Cc}|\p{Cf}/u.test(value) ||
				value.includes("\\") ||
				!URL.canParse(value)
			) {
				return `Expected a canonical ${description}`;
			}

			const url = new URL(value);

			return url.protocol === "https:" &&
				url.username.length === 0 &&
				url.password.length === 0 &&
				url.search.length === 0 &&
				url.hash.length === 0 &&
				(!require_public_host || is_public_hostname(url.hostname))
				? undefined
				: `Expected a credential-free ${description}`;
		}),
	);

const is_public_hostname = (hostname: string) => {
	const normalized = hostname
		.toLowerCase()
		.replace(/^\[|\]$/g, "")
		.replace(/\.$/, "");

	if (
		normalized.length === 0 ||
		normalized === "localhost" ||
		normalized.endsWith(".localhost")
	) {
		return false;
	}

	if (!ipaddr.isValid(normalized)) {
		return true;
	}

	return ipaddr.process(normalized).range() === "unicast";
};

const is_same_artifact_identity = (
	left: MarketplaceArtifactIdentity,
	right: MarketplaceArtifactIdentity,
) =>
	left.version === right.version &&
	left.source.kind === right.source.kind &&
	left.source.locator === right.source.locator &&
	left.source.revision === right.source.revision;

const has_unique_names = (items: ReadonlyArray<{ readonly name: string }>) =>
	new Set(items.map((item) => item.name)).size === items.length;

/** Selects where a Marketplace entry may be installed or connected. */
export const MarketplaceScope = Schema.Union([
	Schema.Struct({ kind: Schema.Literal("global") }),
	Schema.Struct({ kind: Schema.Literal("workspace"), workspace_id: BoundedIdentifier }),
	Schema.Struct({ kind: Schema.Literal("project"), project_id: BoundedIdentifier }),
]);

export type MarketplaceScope = typeof MarketplaceScope.Type;

/** Classifies the origin of a provider-neutral Marketplace entry. */
export const MarketplaceSourceKind = Schema.Literals([
	"local",
	"git",
	"package",
	"catalog",
	"provider",
	"plugin",
]);

export type MarketplaceSourceKind = typeof MarketplaceSourceKind.Type;

/** Validates one revision token without accepting flags, assignments, or credential material. */
export const MarketplaceSourceRevision = Schema.String.check(
	Schema.isPattern(/^(?:[a-f0-9]{40}|[a-f0-9]{64})$/, {
		message: "Expected a lowercase SHA-1 or SHA-256 source revision",
	}),
);

export type MarketplaceSourceRevision = typeof MarketplaceSourceRevision.Type;

const LocalSourceLocator = bounded_text(1_024, "a local source locator").check(
	Schema.isPattern(/^[A-Za-z0-9@._+-]+(?:\/[A-Za-z0-9@._+-]+)*$/),
	Schema.makeFilter<string>((value) =>
		value.split("/").some((segment) => segment === "." || segment === "..")
			? "Expected a canonical relative local source locator"
			: undefined,
	),
);
const GitSourceLocator = credential_free_https_url("HTTPS Git source locator", false);
const PackageSourceLocator = Schema.String.check(
	Schema.isMaxLength(256),
	Schema.isPattern(/^(?:@[a-z0-9][a-z0-9._-]*\/)?[a-z0-9][a-z0-9._-]*$/),
);
const RegistrySourceLocator = Schema.String.check(
	Schema.isMaxLength(256),
	Schema.isPattern(/^[a-z][a-z0-9]*(?:[._/-][a-z0-9]+)*$/),
);
const MarketplaceSourceFields = {
	display_name: VisibleName,
	revision: Schema.optional(MarketplaceSourceRevision),
};

/** Describes attributable source metadata without provider-native configuration. */
export const MarketplaceSource = Schema.Union([
	Schema.Struct({
		...MarketplaceSourceFields,
		kind: Schema.Literal("local"),
		locator: LocalSourceLocator,
	}),
	Schema.Struct({
		...MarketplaceSourceFields,
		kind: Schema.Literal("git"),
		locator: GitSourceLocator,
	}),
	Schema.Struct({
		...MarketplaceSourceFields,
		kind: Schema.Literal("package"),
		locator: PackageSourceLocator,
	}),
	Schema.Struct({
		...MarketplaceSourceFields,
		kind: Schema.Literal("catalog"),
		locator: RegistrySourceLocator,
	}),
	Schema.Struct({
		...MarketplaceSourceFields,
		kind: Schema.Literal("provider"),
		locator: RegistrySourceLocator,
	}),
	Schema.Struct({
		...MarketplaceSourceFields,
		kind: Schema.Literal("plugin"),
		locator: RegistrySourceLocator,
	}),
]);

export type MarketplaceSource = typeof MarketplaceSource.Type;

/** Validates a bounded semantic version used by canonical Marketplace artifacts. */
export const MarketplaceSemanticVersion = Schema.String.check(
	Schema.isMaxLength(128),
	Schema.isPattern(
		/^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/,
		{ message: "Expected a semantic version within 128 characters" },
	),
);

export type MarketplaceSemanticVersion = typeof MarketplaceSemanticVersion.Type;

/** Binds one Marketplace artifact version to its attributable canonical source. */
export const MarketplaceArtifactIdentity = Schema.Struct({
	source: MarketplaceSource,
	version: MarketplaceSemanticVersion,
});

export type MarketplaceArtifactIdentity = typeof MarketplaceArtifactIdentity.Type;

/** Records the user-visible trust assessment required before Marketplace side effects. */
export const MarketplaceTrust = Schema.Struct({
	level: Schema.Literals(["unverified", "community", "verified", "local", "provider_imported"]),
	reasons: BoundedCollection(VisibleSummary),
});

export type MarketplaceTrust = typeof MarketplaceTrust.Type;

/** Enumerates bounded user-understandable Marketplace permissions. */
export const MarketplacePermission = Schema.Struct({
	kind: Schema.Literals([
		"filesystem_read",
		"filesystem_write",
		"process_start",
		"network_connect",
		"browser_access",
		"account_access",
		"secret_reference",
	]),
	label: VisibleName,
	required: Schema.Boolean,
});

export type MarketplacePermission = typeof MarketplacePermission.Type;

/** Identifies a V1 engine that can receive an Artisan Marketplace mirror. */
export const MarketplaceEngine = Schema.Literals(["codex", "claude"]);

export type MarketplaceEngine = typeof MarketplaceEngine.Type;

/** Records an engine-specific mirror and drift state without native configuration. */
export const MarketplaceEngineSync = Schema.Struct({
	drift: Schema.Literals(["none", "detected", "ignored", "resolution_required"]),
	engine: MarketplaceEngine,
	identity: MarketplaceArtifactIdentity,
	last_error_code: Schema.optional(BoundedIdentifier),
	status: Schema.Literals([
		"synced",
		"runtime_only",
		"unsupported",
		"auth_required",
		"sync_failed",
		"drift_detected",
	]),
	updated_at: IsoDateTime,
});

export type MarketplaceEngineSync = typeof MarketplaceEngineSync.Type;

/** Records a canonical routine command without exposing an executable shell command. */
export const RoutineCommand = Schema.Struct({
	command_id: BoundedIdentifier,
	description: VisibleSummary,
	label: VisibleName,
});

export type RoutineCommand = typeof RoutineCommand.Type;

/** Records a required routine file using a workspace-relative destination. */
export const RoutineFile = Schema.Struct({
	path: RelativePath,
	purpose: VisibleSummary,
	write_mode: Schema.Literals(["create", "replace", "merge"]),
});

export type RoutineFile = typeof RoutineFile.Type;

/** Supplies the brief routine data used for discovery before instructions are loaded. */
export const RoutineSummary = Schema.Struct({
	description: VisibleSummary,
	display_name: VisibleName,
	identity: MarketplaceArtifactIdentity,
	routine_id: BoundedIdentifier,
});

export type RoutineSummary = typeof RoutineSummary.Type;

/** Supplies full routine instructions only after a user or agent selects the routine. */
export const RoutineInstructions = Schema.Struct({
	content: bounded_byte_text(
		marketplace_instructions_maximum_bytes,
		"routine instructions",
	).check(Schema.isMinLength(1)),
	content_hash: Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/)),
});

export type RoutineInstructions = typeof RoutineInstructions.Type;

/** Records the canonical Routine registry entry and its Marketplace lifecycle. */
const RoutineFields = {
	commands: BoundedCollection(RoutineCommand),
	compatibility: BoundedCollection(MarketplaceEngine),
	display_name: VisibleName,
	files: BoundedCollection(RoutineFile),
	permissions: BoundedCollection(MarketplacePermission),
	scope: MarketplaceScope,
	summary: RoutineSummary,
	sync: BoundedCollection(MarketplaceEngineSync),
	trust: MarketplaceTrust,
	updated_at: IsoDateTime,
};

const RoutineLifecycle = Schema.Union([
	Schema.Struct({ ...RoutineFields, lifecycle: Schema.Literal("enabled") }),
	Schema.Struct({
		...RoutineFields,
		disabled_reason: VisibleSummary,
		lifecycle: Schema.Literal("disabled"),
	}),
	Schema.Struct({ ...RoutineFields, lifecycle: Schema.Literal("removed") }),
]);

type RoutineLifecycle = typeof RoutineLifecycle.Type;

export const Routine = RoutineLifecycle.check(
	Schema.makeFilter<RoutineLifecycle>((routine) => {
		if (new Set(routine.compatibility).size !== routine.compatibility.length) {
			return "Expected unique Routine compatibility engines";
		}

		if (new Set(routine.sync.map((sync) => sync.engine)).size !== routine.sync.length) {
			return "Expected one Routine sync row per engine";
		}

		return routine.sync.every((sync) =>
			is_same_artifact_identity(sync.identity, routine.summary.identity),
		)
			? undefined
			: "Expected every Routine sync state to reference the canonical source and version";
	}),
);

export type Routine = typeof Routine.Type;

/** Describes a reversible plan to restore Marketplace-managed local state. */
export const MarketplaceRollbackPlan = Schema.Struct({
	actions: BoundedCollection(VisibleSummary),
	available: Schema.Boolean,
	identity: MarketplaceArtifactIdentity,
	rollback_id: Schema.optional(BoundedIdentifier),
});

export type MarketplaceRollbackPlan = typeof MarketplaceRollbackPlan.Type;

/** Shows the exact Routine source, writes, permissions, and compatibility before installation. */
const RoutineInstallPreviewFields = {
	compatibility: BoundedCollection(MarketplaceEngine),
	files: BoundedCollection(RoutineFile),
	identity: MarketplaceArtifactIdentity,
	permissions: BoundedCollection(MarketplacePermission),
	rollback: MarketplaceRollbackPlan,
	scope: MarketplaceScope,
	trust: MarketplaceTrust,
};

const RoutineInstallPreviewUnchecked = Schema.Struct(RoutineInstallPreviewFields);

type RoutineInstallPreviewUnchecked = typeof RoutineInstallPreviewUnchecked.Type;

export const RoutineInstallPreview = RoutineInstallPreviewUnchecked.check(
	Schema.makeFilter<RoutineInstallPreviewUnchecked>((preview) =>
		is_same_artifact_identity(preview.identity, preview.rollback.identity)
			? undefined
			: "Expected the rollback plan to reference the approved Routine source and version",
	),
);

export type RoutineInstallPreview = typeof RoutineInstallPreview.Type;

/** Represents a Routine installation approval lifecycle without execution details. */
export const RoutineInstallApproval = Schema.Struct({
	approval_id: BoundedIdentifier,
	decision: Schema.Literals([
		"pending",
		"approved",
		"denied",
		"applied",
		"failed",
		"rolled_back",
	]),
	preview: RoutineInstallPreview,
	routine_id: BoundedIdentifier,
	updated_at: IsoDateTime,
});

export type RoutineInstallApproval = typeof RoutineInstallApproval.Type;

/** References a secure-store secret by stable name without carrying secret material. */
export const McpSecretReference = Schema.Struct({
	purpose: VisibleSummary,
	secret_id: Schema.String.check(
		Schema.isMaxLength(256),
		Schema.isPattern(/^[A-Za-z][A-Za-z0-9._/-]*$/),
	),
});

export type McpSecretReference = typeof McpSecretReference.Type;

/** Injects one secure-store secret into a stdio process environment by reference. */
export const McpStdioSecretEnvironment = Schema.Struct({
	name: Schema.String.check(
		Schema.isMaxLength(128),
		Schema.isPattern(/^[A-Za-z_][A-Za-z0-9_]*$/),
	),
	secret: McpSecretReference,
});

export type McpStdioSecretEnvironment = typeof McpStdioSecretEnvironment.Type;

/** Validates one executable name or forward-slash path without embedded arguments. */
export const McpStdioCommand = Schema.String.check(
	Schema.isMaxLength(1_024),
	Schema.isPattern(/^(?:(?:[A-Za-z]:)?\/)?[A-Za-z0-9._+-]+(?:\/[A-Za-z0-9._+-]+)*$/),
	Schema.makeFilter<string>((value) =>
		value.split("/").some((segment) => segment === "." || segment === "..")
			? "Expected an executable without relative path traversal"
			: undefined,
	),
);

export type McpStdioCommand = typeof McpStdioCommand.Type;

const SensitiveOptionSegment = Schema.makeFilter<string>((value) =>
	/(?:^|-)(?:api-key|apikey|auth|authentication|authorization|basic|bearer|credential|credentials|env|environment|key|oauth|password|passwd|secret|token|user|username)(?:-|$)/u.test(
		value,
	)
		? "Expected a non-sensitive stdio option name"
		: undefined,
);

/** Validates a canonical non-sensitive stdio option name without CLI punctuation. */
export const McpStdioOptionName = Schema.String.check(
	Schema.isMaxLength(128),
	Schema.isPattern(/^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$/),
	SensitiveOptionSegment,
);

export type McpStdioOptionName = typeof McpStdioOptionName.Type;

const StdioArgumentValue = Schema.String.check(
	Schema.isMinLength(1),
	Schema.isMaxLength(1_024),
	Schema.makeFilter<string>((value) => {
		const is_number = /^-?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/u.test(value);

		if (
			value !== value.trim() ||
			/\p{Cc}|\p{Cf}/u.test(value) ||
			value.includes("\\") ||
			value.includes("=") ||
			value.startsWith("//") ||
			(value.startsWith("-") && !is_number) ||
			/^[^\s/:@]+:[^\s/@]+@/u.test(value) ||
			/(?:^|[\s._:/=-])(?:api[-_]?key|apikey|auth(?:entication|orization)?|basic|bearer|credential|credentials|oauth|password|passwd|secret|token|username)(?:[\s._:/=-]|$)/iu.test(
				value,
			)
		) {
			return "Expected a canonical non-sensitive stdio argument value";
		}

		if (!/^[A-Za-z][A-Za-z0-9+.-]*:\/\//u.test(value)) {
			return undefined;
		}

		if (!URL.canParse(value)) {
			return "Expected a canonical stdio argument URL";
		}

		const url = new URL(value);

		return url.username.length === 0 &&
			url.password.length === 0 &&
			url.search.length === 0 &&
			url.hash.length === 0
			? undefined
			: "Expected a credential-free stdio argument URL";
	}),
);

/** Represents one positional, flag, or option-value stdio argument. */
export const McpStdioArgument = Schema.Union([
	Schema.Struct({ kind: Schema.Literal("positional"), value: StdioArgumentValue }),
	Schema.Struct({ kind: Schema.Literal("option"), name: McpStdioOptionName }),
	Schema.Struct({
		kind: Schema.Literal("option_value"),
		name: McpStdioOptionName,
		value: StdioArgumentValue,
	}),
]);

export type McpStdioArgument = typeof McpStdioArgument.Type;

/** Identifies how an MCP capability communicates with Artisan. */
const McpStdioTransportUnchecked = Schema.Struct({
	arguments: BoundedCollection(McpStdioArgument),
	command: McpStdioCommand,
	environment: BoundedCollection(McpStdioSecretEnvironment),
	kind: Schema.Literal("stdio"),
	working_directory: Schema.optional(RelativePath),
});

type McpStdioTransportUnchecked = typeof McpStdioTransportUnchecked.Type;

export const McpStdioTransport = McpStdioTransportUnchecked.check(
	Schema.makeFilter<McpStdioTransportUnchecked>((transport) =>
		new Set(transport.environment.map((entry) => entry.name.toUpperCase())).size ===
		transport.environment.length
			? undefined
			: "Expected unique stdio environment names",
	),
);

export type McpStdioTransport = typeof McpStdioTransport.Type;

type McpNetworkScope = "none" | "localhost" | "remote";

const is_loopback_hostname = (hostname: string) => {
	const normalized = hostname
		.toLowerCase()
		.replace(/^\[|\]$/g, "")
		.replace(/\.$/, "");

	if (normalized === "localhost" || normalized.endsWith(".localhost")) {
		return true;
	}

	return ipaddr.isValid(normalized) && ipaddr.process(normalized).range() === "loopback";
};

const stdio_network_scope = (transport: McpStdioTransport): McpNetworkScope => {
	let scope: McpNetworkScope = "none";

	for (const argument of transport.arguments) {
		if (!("value" in argument) || !/^[A-Za-z][A-Za-z0-9+.-]*:\/\//u.test(argument.value)) {
			continue;
		}

		const url = new URL(argument.value);

		if (url.hostname.length === 0) {
			continue;
		}

		if (!is_loopback_hostname(url.hostname)) {
			return "remote";
		}

		scope = "localhost";
	}

	return scope;
};

/** Describes one credential-free public Streamable HTTP transport. */
export const McpStreamableHttpTransport = Schema.Struct({
	endpoint: credential_free_https_url("Streamable HTTP endpoint on a public host", true),
	kind: Schema.Literal("streamable_http"),
});

export type McpStreamableHttpTransport = typeof McpStreamableHttpTransport.Type;

/** Identifies how an MCP capability communicates with Artisan. */
export const McpTransport = Schema.Union([McpStdioTransport, McpStreamableHttpTransport]);

export type McpTransport = typeof McpTransport.Type;

/** Describes MCP authentication through references and public OAuth state only. */
export const McpAuth = Schema.Union([
	Schema.Struct({ kind: Schema.Literal("none") }),
	Schema.Struct({ kind: Schema.Literal("bearer"), secret: McpSecretReference }),
	Schema.Struct({ kind: Schema.Literal("api_key"), secret: McpSecretReference }),
	Schema.Struct({
		kind: Schema.Literal("oauth"),
		scopes: BoundedCollection(bounded_text(256, "an OAuth scope")),
		status: Schema.Literals([
			"not_started",
			"authorization_required",
			"authorized",
			"refresh_required",
		]),
	}),
]);

export type McpAuth = typeof McpAuth.Type;

/** Validates one canonical lowercase MCP machine name. */
export const McpMachineName = Schema.String.check(
	Schema.isMaxLength(128),
	Schema.isPattern(/^[a-z][a-z0-9]*(?:[._-][a-z0-9]+)*$/),
);

export type McpMachineName = typeof McpMachineName.Type;

/** Describes one discoverable MCP surface without tool schemas or provider payloads. */
export const McpExposedItem = Schema.Struct({
	description: Schema.optional(VisibleSummary),
	name: McpMachineName,
});

export type McpExposedItem = typeof McpExposedItem.Type;

/** Describes one MCP tool's execution policy. */
export const McpToolPolicy = Schema.Struct({
	approval_policy: Schema.Literals(["automatic", "required"]),
	label: VisibleName,
	status: Schema.Literals(["allowed", "disabled"]),
	tool_name: McpMachineName,
});

export type McpToolPolicy = typeof McpToolPolicy.Type;

/** Records the canonical MCP capability registry entry and runtime state. */
const McpCapabilityFields = {
	auth: McpAuth,
	compatibility: BoundedCollection(MarketplaceEngine),
	display_name: VisibleName,
	health: Schema.Literals([
		"unknown",
		"healthy",
		"degraded",
		"unhealthy",
		"authentication_required",
	]),
	identity: MarketplaceArtifactIdentity,
	instructions: Schema.optional(RoutineInstructions),
	permissions: BoundedCollection(MarketplacePermission),
	prompts: BoundedCollection(McpExposedItem),
	resources: BoundedCollection(McpExposedItem),
	scope: MarketplaceScope,
	sync: BoundedCollection(MarketplaceEngineSync),
	tools: BoundedCollection(McpExposedItem),
	tool_policy: BoundedCollection(McpToolPolicy),
	transport: McpTransport,
	trust: MarketplaceTrust,
	updated_at: IsoDateTime,
	capability_id: BoundedIdentifier,
};

const McpCapabilityLifecycle = Schema.Union([
	Schema.Struct({ ...McpCapabilityFields, lifecycle: Schema.Literal("enabled") }),
	Schema.Struct({
		...McpCapabilityFields,
		disabled_reason: VisibleSummary,
		lifecycle: Schema.Literal("disabled"),
	}),
	Schema.Struct({ ...McpCapabilityFields, lifecycle: Schema.Literal("removed") }),
	Schema.Struct({ ...McpCapabilityFields, lifecycle: Schema.Literal("connecting") }),
	Schema.Struct({ ...McpCapabilityFields, lifecycle: Schema.Literal("connected") }),
	Schema.Struct({ ...McpCapabilityFields, lifecycle: Schema.Literal("crashed") }),
]);

type McpCapabilityLifecycle = typeof McpCapabilityLifecycle.Type;

export const McpCapability = McpCapabilityLifecycle.check(
	Schema.makeFilter<McpCapabilityLifecycle>((capability) => {
		const tool_names = new Set(capability.tools.map((tool) => tool.name));
		const policy_names = new Set(capability.tool_policy.map((policy) => policy.tool_name));
		const sync_engines = new Set(capability.sync.map((sync) => sync.engine));

		if (new Set(capability.compatibility).size !== capability.compatibility.length) {
			return "Expected unique MCP compatibility engines";
		}

		if (sync_engines.size !== capability.sync.length) {
			return "Expected one MCP sync row per engine";
		}

		if (!has_unique_names(capability.tools)) {
			return "Expected unique exposed MCP tool names";
		}

		if (!has_unique_names(capability.resources)) {
			return "Expected unique exposed MCP resource names";
		}

		if (!has_unique_names(capability.prompts)) {
			return "Expected unique exposed MCP prompt names";
		}

		if (policy_names.size !== capability.tool_policy.length) {
			return "Expected exactly one policy for each MCP tool";
		}

		if (
			policy_names.size !== tool_names.size ||
			[...policy_names].some((tool_name) => !tool_names.has(tool_name))
		) {
			return "Expected MCP policies to cover every exposed tool and no unknown tools";
		}

		if (
			capability.transport.kind === "streamable_http" &&
			!capability.permissions.some((permission) => permission.kind === "network_connect")
		) {
			return "Expected Streamable HTTP capabilities to disclose network access";
		}

		if (
			capability.transport.kind === "stdio" &&
			stdio_network_scope(capability.transport) !== "none" &&
			!capability.permissions.some((permission) => permission.kind === "network_connect")
		) {
			return "Expected networked stdio capabilities to disclose network access";
		}

		return capability.sync.every((sync) =>
			is_same_artifact_identity(sync.identity, capability.identity),
		)
			? undefined
			: "Expected every MCP sync state to reference the canonical source and version";
	}),
);

export type McpCapability = typeof McpCapability.Type;

/** Shows source, connection target, auth references, network access, and tools before an MCP starts. */
const McpConnectPreviewUnchecked = Schema.Struct({
	capability: McpCapability,
	network: Schema.Literals(["none", "localhost", "remote"]),
});

type McpConnectPreviewUnchecked = typeof McpConnectPreviewUnchecked.Type;

export const McpConnectPreview = McpConnectPreviewUnchecked.check(
	Schema.makeFilter<McpConnectPreviewUnchecked>((preview) => {
		const has_network_permission = preview.capability.permissions.some(
			(permission) => permission.kind === "network_connect",
		);

		if (
			preview.capability.transport.kind === "streamable_http" &&
			preview.network !== "remote"
		) {
			return "Expected Streamable HTTP connection previews to use remote networking";
		}

		if (preview.capability.transport.kind === "stdio") {
			const transport_scope = stdio_network_scope(preview.capability.transport);

			if (transport_scope !== "none" && preview.network !== transport_scope) {
				return "Expected explicit stdio endpoints to match their network scope";
			}
		}

		return (preview.network === "none") === has_network_permission
			? "Expected connection preview networking to match the disclosed permissions"
			: undefined;
	}),
);

export type McpConnectPreview = typeof McpConnectPreview.Type;

/** Represents MCP connection approval or disconnect state without secrets or native diagnostics. */
const McpConnectionApprovalUnchecked = Schema.Struct({
	approval_id: BoundedIdentifier,
	capability_id: BoundedIdentifier,
	decision: Schema.Literals([
		"pending",
		"approved",
		"denied",
		"connecting",
		"connected",
		"disconnected",
		"failed",
	]),
	preview: McpConnectPreview,
	updated_at: IsoDateTime,
});

type McpConnectionApprovalUnchecked = typeof McpConnectionApprovalUnchecked.Type;

export const McpConnectionApproval = McpConnectionApprovalUnchecked.check(
	Schema.makeFilter<McpConnectionApprovalUnchecked>((approval) =>
		approval.capability_id === approval.preview.capability.capability_id
			? undefined
			: "Expected the approval to reference its fully disclosed MCP capability",
	),
);

export type McpConnectionApproval = typeof McpConnectionApproval.Type;

/** Queries Marketplace entries that can be used in one engine and scope. */
export const MarketplaceEligibilityQuery = Schema.Struct({
	engine: MarketplaceEngine,
	scope: MarketplaceScope,
});

export type MarketplaceEligibilityQuery = typeof MarketplaceEligibilityQuery.Type;

/** Reports one eligible or ineligible Marketplace entry without invoking it. */
export const MarketplaceEligibilityResult = Schema.Struct({
	entry_id: BoundedIdentifier,
	entry_kind: Schema.Literals(["routine", "mcp"]),
	reason_code: Schema.optional(BoundedIdentifier),
	state: Schema.Literals([
		"eligible",
		"disabled",
		"incompatible",
		"permission_required",
		"auth_required",
		"removed",
	]),
});

export type MarketplaceEligibilityResult = typeof MarketplaceEligibilityResult.Type;

/** Records a source-safe Marketplace lifecycle transition for later EventPayload integration. */
export const MarketplaceLifecycleEvent = Schema.Struct({
	entry_id: BoundedIdentifier,
	entry_kind: Schema.Literals(["routine", "mcp"]),
	lifecycle: Schema.Literals([
		"enabled",
		"disabled",
		"removed",
		"connecting",
		"connected",
		"disconnected",
		"crashed",
	]),
	summary: Schema.optional(VisibleSummary),
	type: Schema.Literal("marketplace.lifecycle.updated"),
	updated_at: IsoDateTime,
});

export type MarketplaceLifecycleEvent = typeof MarketplaceLifecycleEvent.Type;

/** Records a source-safe Routine or MCP invocation outcome without arguments, results, or credentials. */
export const MarketplaceInvocationEvent = Schema.Struct({
	approval_required: Schema.Boolean,
	entry_id: BoundedIdentifier,
	entry_kind: Schema.Literals(["routine", "mcp"]),
	invocation_id: BoundedIdentifier,
	state: Schema.Literals([
		"started",
		"approval_required",
		"completed",
		"failed",
		"denied",
		"outcome_unknown",
	]),
	summary: Schema.optional(VisibleSummary),
	type: Schema.Literal("marketplace.invocation.updated"),
});

export type MarketplaceInvocationEvent = typeof MarketplaceInvocationEvent.Type;

/** Preserves the bounded number of source-safe events that one Marketplace batch may carry. */
export const MarketplaceEventBatch = Schema.Struct({
	events: BoundedCollection(
		Schema.Union([MarketplaceLifecycleEvent, MarketplaceInvocationEvent]),
	),
	schema_version: Schema.Literal(1),
});

export type MarketplaceEventBatch = typeof MarketplaceEventBatch.Type;

const strict_decoder = <S extends Schema.Constraint>(schema: S) => {
	const Decode = Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" });

	return (input: unknown) => Decode(input);
};

/** Strictly decodes one attributable Marketplace source. */
export const DecodeMarketplaceSource = strict_decoder(MarketplaceSource);

/** Strictly decodes one canonical source and semantic-version identity. */
export const DecodeMarketplaceArtifactIdentity = strict_decoder(MarketplaceArtifactIdentity);

/** Strictly decodes one engine-specific Marketplace sync projection. */
export const DecodeMarketplaceEngineSync = strict_decoder(MarketplaceEngineSync);

/** Strictly decodes progressive-disclosure Routine instructions. */
export const DecodeRoutineInstructions = strict_decoder(RoutineInstructions);

/** Strictly decodes one canonical Routine projection. */
export const DecodeRoutine = strict_decoder(Routine);

/** Strictly decodes one source-bound Routine rollback projection. */
export const DecodeMarketplaceRollbackPlan = strict_decoder(MarketplaceRollbackPlan);

/** Strictly decodes one Routine installation preview. */
export const DecodeRoutineInstallPreview = strict_decoder(RoutineInstallPreview);

/** Strictly decodes one Routine installation approval. */
export const DecodeRoutineInstallApproval = strict_decoder(RoutineInstallApproval);

/** Strictly decodes one credential-safe MCP transport. */
export const DecodeMcpTransport = strict_decoder(McpTransport);

/** Strictly decodes one reference-only MCP authentication projection. */
export const DecodeMcpAuth = strict_decoder(McpAuth);

/** Strictly decodes one canonical MCP Capability projection. */
export const DecodeMcpCapability = strict_decoder(McpCapability);

/** Strictly decodes one source-bound MCP connection preview. */
export const DecodeMcpConnectPreview = strict_decoder(McpConnectPreview);

/** Strictly decodes one MCP connection approval. */
export const DecodeMcpConnectionApproval = strict_decoder(McpConnectionApproval);

/** Strictly decodes one Marketplace eligibility query. */
export const DecodeMarketplaceEligibilityQuery = strict_decoder(MarketplaceEligibilityQuery);

/** Strictly decodes one Marketplace eligibility result. */
export const DecodeMarketplaceEligibilityResult = strict_decoder(MarketplaceEligibilityResult);

/** Strictly decodes one source-safe Marketplace lifecycle event. */
export const DecodeMarketplaceLifecycleEvent = strict_decoder(MarketplaceLifecycleEvent);

/** Strictly decodes one source-safe Marketplace invocation event. */
export const DecodeMarketplaceInvocationEvent = strict_decoder(MarketplaceInvocationEvent);

/** Strictly decodes one bounded source-safe Marketplace event batch. */
export const DecodeMarketplaceEventBatch = strict_decoder(MarketplaceEventBatch);
