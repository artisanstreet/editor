import { Schema } from "effect";

import { Identifier, IsoDateTime } from "./common";

/** Canonical installation boundary; provider-native files remain mirrors. */
/** Carries the exact boundary used for eligibility; scoped records are never ambiguous strings. */
export const MarketplaceScope = Schema.Union([
	Schema.Struct({ kind: Schema.Literal("global") }),
	Schema.Struct({ kind: Schema.Literal("workspace"), workspace_id: Identifier }),
	Schema.Struct({ kind: Schema.Literal("project"), project_id: Identifier }),
]);
export type MarketplaceScope = typeof MarketplaceScope.Type;

export const MarketplaceItemStatus = Schema.Literals([
	"preview",
	"awaiting_approval",
	"installing",
	"connecting",
	"approval_denied",
	"enabled",
	"disabled",
	"failed",
	"removed",
	"rolled_back",
	"rollback_available",
	"uninstall_available",
	"disconnect_available",
]);
export type MarketplaceItemStatus = typeof MarketplaceItemStatus.Type;

/** Renderer-safe browser/filter contract shared by routine and capability registries. */
export const MarketplaceBrowseQuery = Schema.Struct({
	category: Schema.optional(Schema.Literals(["routine", "capability"])),
	compatibility_engine_id: Schema.optional(Identifier),
	enabled: Schema.optional(Schema.Boolean),
	scope: Schema.optional(MarketplaceScope),
	status: Schema.optional(MarketplaceItemStatus),
	text: Schema.optional(Schema.NonEmptyString),
});
export type MarketplaceBrowseQuery = typeof MarketplaceBrowseQuery.Type;

export const MarketplaceSourceKind = Schema.Literals([
	"local",
	"git",
	"package_manager",
	"catalog",
	"provider_import",
	"plugin_bundle",
]);
export type MarketplaceSourceKind = typeof MarketplaceSourceKind.Type;

/** Identifies an opaque OS-backed secret entry; its value never crosses this protocol. */
export const SecretReference = Schema.Struct({
	provider: Schema.NonEmptyString,
	secret_id: Identifier,
});
export type SecretReference = typeof SecretReference.Type;

export const MarketplacePermission = Schema.Struct({
	description: Schema.NonEmptyString,
	kind: Schema.Literals([
		"filesystem_read",
		"filesystem_write",
		"network",
		"process",
		"terminal",
		"browser",
		"account",
		"database",
		"environment",
	]),
});
export type MarketplacePermission = typeof MarketplacePermission.Type;

export const MarketplaceTrust = Schema.Literals(["verified", "known", "unverified", "local"]);
export type MarketplaceTrust = typeof MarketplaceTrust.Type;

export const EngineCompatibility = Schema.Struct({
	engine_id: Identifier,
	minimum_version: Schema.optional(Schema.NonEmptyString),
	state: Schema.Literals(["native", "runtime_only", "unsupported"]),
});
export type EngineCompatibility = typeof EngineCompatibility.Type;

export const ProviderSyncStatus = Schema.Literals([
	"synced",
	"runtime_only",
	"unsupported",
	"auth_required",
	"sync_failed",
	"drift_detected",
	"drift_ignored",
]);
export type ProviderSyncStatus = typeof ProviderSyncStatus.Type;

export const ProviderSyncState = Schema.Struct({
	engine_id: Identifier,
	last_error_code: Schema.optional(Identifier),
	observed_revision: Schema.optional(Schema.NonEmptyString),
	status: ProviderSyncStatus,
	updated_at: IsoDateTime,
});
export type ProviderSyncState = typeof ProviderSyncState.Type;

export const DriftResolution = Schema.Literals(["import", "overwrite", "ignore"]);
export type DriftResolution = typeof DriftResolution.Type;

export const RoutineSource = Schema.Struct({
	kind: MarketplaceSourceKind,
	locator: Schema.NonEmptyString,
	provider: Schema.optional(Identifier),
	revision: Schema.optional(Schema.NonEmptyString),
});
export type RoutineSource = typeof RoutineSource.Type;

export const RoutineFile = Schema.Struct({
	path: Schema.NonEmptyString,
	required: Schema.Boolean,
});
export type RoutineFile = typeof RoutineFile.Type;

export const RoutineCommand = Schema.Struct({
	description: Schema.NonEmptyString,
	name: Schema.NonEmptyString,
});
export type RoutineCommand = typeof RoutineCommand.Type;

/** Lightweight routine metadata used for discovery; instructions are intentionally absent. */
export const RoutineSummary = Schema.Struct({
	description: Schema.NonEmptyString,
	display_name: Schema.NonEmptyString,
	enabled: Schema.Boolean,
	id: Identifier,
	scope: MarketplaceScope,
	status: MarketplaceItemStatus,
	version: Schema.NonEmptyString,
});
export type RoutineSummary = typeof RoutineSummary.Type;

/** Canonical routine record, disclosed only after selection or invocation. */
export const RoutineDetail = Schema.Struct({
	author: Schema.optional(Schema.NonEmptyString),
	compatibility: Schema.Array(EngineCompatibility),
	description: Schema.NonEmptyString,
	display_name: Schema.NonEmptyString,
	enabled: Schema.Boolean,
	exported_commands: Schema.Array(RoutineCommand),
	files: Schema.Array(RoutineFile),
	id: Identifier,
	instructions: Schema.NonEmptyString,
	permissions: Schema.Array(MarketplacePermission),
	removed_at: Schema.optional(IsoDateTime),
	scope: MarketplaceScope,
	status: MarketplaceItemStatus,
	source: RoutineSource,
	sync: Schema.Array(ProviderSyncState),
	trust: MarketplaceTrust,
	version: Schema.NonEmptyString,
});
export type RoutineDetail = typeof RoutineDetail.Type;

export const RoutineInstallPreviewRequest = Schema.Struct({
	scope: MarketplaceScope,
	source: RoutineSource,
});
export type RoutineInstallPreviewRequest = typeof RoutineInstallPreviewRequest.Type;

export const RoutineInstallPreview = Schema.Struct({
	candidate_id: Identifier,
	candidate_name: Schema.NonEmptyString,
	compatibility: Schema.Array(EngineCompatibility),
	files: Schema.Array(RoutineFile),
	permissions: Schema.Array(MarketplacePermission),
	preview_fingerprint: Identifier,
	rollback_available: Schema.Boolean,
	scope: MarketplaceScope,
	source: RoutineSource,
	trust: MarketplaceTrust,
	version: Schema.NonEmptyString,
});
export type RoutineInstallPreview = typeof RoutineInstallPreview.Type;

export const RoutineInstallRequest = Schema.Struct({
	approval_id: Identifier,
	preview_fingerprint: Identifier,
	requested_by: Schema.Literals(["user", "agent"]),
	scope: MarketplaceScope,
	source: RoutineSource,
});
export type RoutineInstallRequest = typeof RoutineInstallRequest.Type;

export const MarketplaceApprovalDecision = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
	preview_fingerprint: Identifier,
});
export type MarketplaceApprovalDecision = typeof MarketplaceApprovalDecision.Type;

export const RoutineDriftResolutionRequest = Schema.Struct({
	action: Schema.Literals(["import", "ignore"]),
	engine_id: Identifier,
	observed_revision: Schema.NonEmptyString,
	routine_id: Identifier,
	scope: MarketplaceScope,
});
export type RoutineDriftResolutionRequest = typeof RoutineDriftResolutionRequest.Type;

/** Exact destructive provider-overwrite intent reviewed before approval is recorded. */
export const RoutineDriftOverwriteRequest = Schema.Struct({
	approval_id: Identifier,
	engine_id: Identifier,
	intent_fingerprint: Identifier,
	observed_revision: Schema.NonEmptyString,
	requested_by: Schema.Literals(["user", "agent"]),
	routine_id: Identifier,
	scope: MarketplaceScope,
});
export type RoutineDriftOverwriteRequest = typeof RoutineDriftOverwriteRequest.Type;
export const RoutineDriftOverwriteDecision = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
	engine_id: Identifier,
	intent_fingerprint: Identifier,
	observed_revision: Schema.NonEmptyString,
	routine_id: Identifier,
	scope: MarketplaceScope,
});
export type RoutineDriftOverwriteDecision = typeof RoutineDriftOverwriteDecision.Type;

export const RoutineInvocationRequest = Schema.Struct({
	command: Schema.optional(Schema.NonEmptyString),
	routine_id: Identifier,
	scope: MarketplaceScope,
	task_summary: Schema.NonEmptyString,
});
export type RoutineInvocationRequest = typeof RoutineInvocationRequest.Type;

export const RoutineInvocationMetadata = Schema.Struct({
	eligible: Schema.Boolean,
	eligibility_reason: Schema.NonEmptyString,
	invocation_id: Identifier,
	routine_id: Identifier,
	version: Schema.NonEmptyString,
});
export type RoutineInvocationMetadata = typeof RoutineInvocationMetadata.Type;

export const RoutineRegistrySnapshot = Schema.Struct({
	registry_version: Schema.Literal(1),
	routines: Schema.Array(RoutineSummary),
});
export type RoutineRegistrySnapshot = typeof RoutineRegistrySnapshot.Type;

/** Inspected npx-skills candidates are import inputs, never canonical routine records. */
export const NpxSkillsDiscoveryRequest = Schema.Struct({
	package_spec: Schema.NonEmptyString,
	scope: MarketplaceScope,
});
export type NpxSkillsDiscoveryRequest = typeof NpxSkillsDiscoveryRequest.Type;
export const NpxSkillsCandidate = Schema.Struct({
	description: Schema.optional(Schema.NonEmptyString),
	files: Schema.Array(RoutineFile),
	name: Schema.NonEmptyString,
	/** Opaque backend inspection identity used by the explicit import preview request. */
	preview_fingerprint: Schema.optional(Identifier),
	source_locator: Schema.NonEmptyString,
	version: Schema.optional(Schema.NonEmptyString),
});
export type NpxSkillsCandidate = typeof NpxSkillsCandidate.Type;
export const NpxSkillsDiscoveryResult = Schema.Struct({
	candidates: Schema.Array(NpxSkillsCandidate),
	package_spec: Schema.NonEmptyString,
});
export type NpxSkillsDiscoveryResult = typeof NpxSkillsDiscoveryResult.Type;
export const NpxSkillsImportRequest = Schema.Struct({
	candidate_name: Schema.NonEmptyString,
	package_spec: Schema.NonEmptyString,
	preview_fingerprint: Identifier,
	scope: MarketplaceScope,
});
export type NpxSkillsImportRequest = typeof NpxSkillsImportRequest.Type;

export const McpTransport = Schema.Union([
	Schema.Struct({
		args: Schema.Array(Schema.String),
		command: Schema.NonEmptyString,
		cwd: Schema.optional(Schema.NonEmptyString),
		env: Schema.optional(
			Schema.Array(
				Schema.Struct({
					name: Schema.NonEmptyString.check(Schema.isPattern(/^[A-Za-z_][A-Za-z0-9_]*$/)),
					secret_ref: SecretReference,
				}),
			),
		),
		invocation_timeout_ms: Schema.optional(
			Schema.Int.check(Schema.isBetween({ minimum: 1, maximum: 300_000 })),
		),
		kind: Schema.Literal("stdio"),
		max_message_bytes: Schema.optional(
			Schema.Int.check(Schema.isBetween({ minimum: 1, maximum: 16 * 1024 * 1024 })),
		),
		max_pending_requests: Schema.optional(
			Schema.Int.check(Schema.isBetween({ minimum: 1, maximum: 1_024 })),
		),
		max_stderr_bytes: Schema.optional(
			Schema.Int.check(Schema.isBetween({ minimum: 1, maximum: 16 * 1024 * 1024 })),
		),
		startup_timeout_ms: Schema.Int.check(Schema.isGreaterThan(0)),
	}),
	Schema.Struct({
		kind: Schema.Literal("streamable_http"),
		max_pagination_bytes: Schema.optional(
			Schema.Int.check(Schema.isBetween({ minimum: 1, maximum: 64 * 1024 * 1024 })),
		),
		max_pagination_items: Schema.optional(
			Schema.Int.check(Schema.isBetween({ minimum: 1, maximum: 100_000 })),
		),
		max_pagination_pages: Schema.optional(
			Schema.Int.check(Schema.isBetween({ minimum: 1, maximum: 1_000 })),
		),
		max_response_bytes: Schema.optional(
			Schema.Int.check(Schema.isBetween({ minimum: 1, maximum: 16 * 1024 * 1024 })),
		),
		timeout_ms: Schema.optional(
			Schema.Int.check(Schema.isBetween({ minimum: 1, maximum: 300_000 })),
		),
		url: Schema.String.check(
			Schema.isPattern(/^https?:\/\//),
			Schema.makeFilter<string>((value) => {
				const url = new URL(value);
				return url.username.length === 0 && url.password.length === 0
					? undefined
					: "Expected an HTTP MCP URL without embedded credentials";
			}),
		),
	}),
]);
export type McpTransport = typeof McpTransport.Type;

export const McpAuth = Schema.Union([
	Schema.Struct({ kind: Schema.Literal("none") }),
	Schema.Struct({ kind: Schema.Literal("bearer"), secret_ref: SecretReference }),
	Schema.Struct({
		header_name: Schema.NonEmptyString.check(
			Schema.isPattern(/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/),
			Schema.makeFilter<string>((value) =>
				new Set([
					"authorization",
					"connection",
					"content-length",
					"cookie",
					"host",
					"mcp-session-id",
					"proxy-authorization",
					"te",
					"trailer",
					"transfer-encoding",
					"upgrade",
				]).has(value.toLowerCase())
					? "Expected a non-reserved API-key header name"
					: undefined,
			),
		),
		kind: Schema.Literal("api_key"),
		secret_ref: SecretReference,
	}),
	Schema.Struct({
		authorization_url: Schema.String.check(
			Schema.isPattern(/^https:\/\//),
			Schema.makeFilter<string>((value) => {
				const url = new URL(value);
				return url.username.length === 0 && url.password.length === 0
					? undefined
					: "Expected an OAuth authorization URL without embedded credentials";
			}),
		),
		kind: Schema.Literal("oauth"),
		provider: Identifier,
		scopes: Schema.Array(Schema.NonEmptyString),
		token_ref: Schema.optional(SecretReference),
		token_status: Schema.Literals([
			"not_started",
			"authorized",
			"refresh_required",
			"expired",
			"failed",
		]),
	}),
]);
export type McpAuth = typeof McpAuth.Type;

export const McpToolPolicy = Schema.Struct({
	approval: Schema.Literals(["never", "always", "sensitive_only"]),
	enabled: Schema.Boolean,
	name: Schema.NonEmptyString,
	sensitive_label: Schema.optional(Schema.NonEmptyString),
});
export type McpToolPolicy = typeof McpToolPolicy.Type;

export const McpToolSummary = Schema.Struct({
	description: Schema.optional(Schema.NonEmptyString),
	input_schema: Schema.optional(Schema.Record(Schema.String, Schema.Unknown)),
	name: Schema.NonEmptyString,
});
export type McpToolSummary = typeof McpToolSummary.Type;

export const McpResourceSummary = Schema.Struct({
	description: Schema.optional(Schema.NonEmptyString),
	uri: Schema.NonEmptyString,
});
export type McpResourceSummary = typeof McpResourceSummary.Type;

export const CapabilityHealth = Schema.Struct({
	checked_at: Schema.optional(IsoDateTime),
	status: Schema.Literals([
		"unknown",
		"healthy",
		"degraded",
		"offline",
		"crashed",
		"auth_required",
	]),
});
export type CapabilityHealth = typeof CapabilityHealth.Type;

export const CapabilityLifecycle = Schema.Literals([
	"disconnected",
	"awaiting_approval",
	"connecting",
	"connected",
	"stopped",
	"crashed",
	"removed",
]);
export type CapabilityLifecycle = typeof CapabilityLifecycle.Type;

export const CapabilitySummary = Schema.Struct({
	display_name: Schema.NonEmptyString,
	enabled: Schema.Boolean,
	health: CapabilityHealth,
	id: Identifier,
	lifecycle: CapabilityLifecycle,
	scope: MarketplaceScope,
	status: MarketplaceItemStatus,
	transport_kind: Schema.Literals(["stdio", "streamable_http"]),
});
export type CapabilitySummary = typeof CapabilitySummary.Type;

export const CapabilityDetail = Schema.Struct({
	auth: McpAuth,
	compatibility: Schema.Array(EngineCompatibility),
	display_name: Schema.NonEmptyString,
	enabled: Schema.Boolean,
	health: CapabilityHealth,
	id: Identifier,
	lifecycle: CapabilityLifecycle,
	permissions: Schema.Array(MarketplacePermission),
	policy: Schema.Array(McpToolPolicy),
	removed_at: Schema.optional(IsoDateTime),
	resources: Schema.Array(McpResourceSummary),
	scope: MarketplaceScope,
	status: MarketplaceItemStatus,
	server_instructions: Schema.optional(Schema.NonEmptyString),
	server_metadata: Schema.optional(Schema.Record(Schema.String, Schema.String)),
	source: RoutineSource,
	sync: Schema.Array(ProviderSyncState),
	tools: Schema.Array(McpToolSummary),
	transport: McpTransport,
	trust: MarketplaceTrust,
	transport_policy: Schema.optional(
		Schema.Struct({
			allowed: Schema.Boolean,
			broad_local_binding_warning: Schema.Boolean,
		}),
	),
});
export type CapabilityDetail = typeof CapabilityDetail.Type;

export const CapabilityConnectPreviewRequest = Schema.Struct({
	auth: McpAuth,
	scope: MarketplaceScope,
	source: RoutineSource,
	transport: McpTransport,
});
export type CapabilityConnectPreviewRequest = typeof CapabilityConnectPreviewRequest.Type;

export const CapabilityConnectPreview = Schema.Struct({
	auth: McpAuth,
	candidate_id: Identifier,
	candidate_name: Schema.NonEmptyString,
	compatibility: Schema.Array(EngineCompatibility),
	discovery_status: Schema.Literals(["declared", "requires_connection"]),
	permissions: Schema.Array(MarketplacePermission),
	preview_fingerprint: Identifier,
	rollback_available: Schema.Boolean,
	scope: MarketplaceScope,
	source: RoutineSource,
	tools: Schema.Array(McpToolSummary),
	transport: McpTransport,
	transport_policy: Schema.optional(
		Schema.Struct({
			allowed: Schema.Boolean,
			broad_local_binding_warning: Schema.Boolean,
		}),
	),
	trust: MarketplaceTrust,
});
export type CapabilityConnectPreview = typeof CapabilityConnectPreview.Type;

export const CapabilityConnectRequest = Schema.Struct({
	approval_id: Identifier,
	auth: McpAuth,
	preview_fingerprint: Identifier,
	requested_by: Schema.Literals(["user", "agent"]),
	scope: MarketplaceScope,
	source: RoutineSource,
	transport: McpTransport,
});
export type CapabilityConnectRequest = typeof CapabilityConnectRequest.Type;

export const RoutineRollbackRequest = Schema.Struct({
	routine_id: Identifier,
	rollback_id: Identifier,
	scope: MarketplaceScope,
});
export type RoutineRollbackRequest = typeof RoutineRollbackRequest.Type;

export const CapabilityOAuthRequest = Schema.Struct({
	capability_id: Identifier,
	scope: MarketplaceScope,
});
export type CapabilityOAuthRequest = typeof CapabilityOAuthRequest.Type;
export const CapabilityOAuthCompleteRequest = Schema.Struct({
	capability_id: Identifier,
	callback_reference: Identifier,
	scope: MarketplaceScope,
});
export type CapabilityOAuthCompleteRequest = typeof CapabilityOAuthCompleteRequest.Type;
export const CapabilityOAuthTokenStatus = Schema.Struct({
	capability_id: Identifier,
	secret_ref: Schema.optional(SecretReference),
	status: Schema.Literals([
		"not_started",
		"authorized",
		"refresh_required",
		"expired",
		"revoked",
		"failed",
	]),
});
export type CapabilityOAuthTokenStatus = typeof CapabilityOAuthTokenStatus.Type;
export const CapabilityOAuthBeginResult = Schema.Struct({
	authorization_url: Schema.NonEmptyString,
	/** Opaque adapter continuation; never an authorization code or access token. */
	continuation_reference: Identifier,
});
export type CapabilityOAuthBeginResult = typeof CapabilityOAuthBeginResult.Type;

export const CapabilityDriftResolutionRequest = Schema.Struct({
	action: Schema.Literals(["import", "ignore"]),
	capability_id: Identifier,
	engine_id: Identifier,
	observed_revision: Schema.NonEmptyString,
	scope: MarketplaceScope,
});
export type CapabilityDriftResolutionRequest = typeof CapabilityDriftResolutionRequest.Type;

/** Exact destructive capability-mirror overwrite intent reviewed before approval. */
export const CapabilityDriftOverwriteRequest = Schema.Struct({
	approval_id: Identifier,
	capability_id: Identifier,
	engine_id: Identifier,
	intent_fingerprint: Identifier,
	observed_revision: Schema.NonEmptyString,
	requested_by: Schema.Literals(["user", "agent"]),
	scope: MarketplaceScope,
});
export type CapabilityDriftOverwriteRequest = typeof CapabilityDriftOverwriteRequest.Type;
export const CapabilityDriftOverwriteDecision = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
	capability_id: Identifier,
	engine_id: Identifier,
	intent_fingerprint: Identifier,
	observed_revision: Schema.NonEmptyString,
	scope: MarketplaceScope,
});
export type CapabilityDriftOverwriteDecision = typeof CapabilityDriftOverwriteDecision.Type;

export const CapabilityInvocationRequest = Schema.Struct({
	arguments_json: Schema.String,
	approval_id: Schema.optional(Identifier),
	capability_id: Identifier,
	scope: MarketplaceScope,
	tool_name: Schema.NonEmptyString,
});
export type CapabilityInvocationRequest = typeof CapabilityInvocationRequest.Type;

/** Exact tool invocation intent; arguments remain opaque JSON and secrets remain references. */
export const CapabilityInvocationApprovalRequest = Schema.Struct({
	approval_id: Identifier,
	arguments_json: Schema.String,
	capability_id: Identifier,
	intent_fingerprint: Identifier,
	requested_by: Schema.Literals(["user", "agent"]),
	scope: MarketplaceScope,
	tool_name: Schema.NonEmptyString,
});
export type CapabilityInvocationApprovalRequest = typeof CapabilityInvocationApprovalRequest.Type;
export const CapabilityInvocationApprovalDecision = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
	arguments_json: Schema.String,
	capability_id: Identifier,
	intent_fingerprint: Identifier,
	scope: MarketplaceScope,
	tool_name: Schema.NonEmptyString,
});
export type CapabilityInvocationApprovalDecision = typeof CapabilityInvocationApprovalDecision.Type;

/** Ledger-safe invocation outcome: result material remains an artifact reference. */
export const CapabilityInvocationMetadata = Schema.Struct({
	approval_required: Schema.Boolean,
	capability_id: Identifier,
	invocation_id: Identifier,
	result_artifact_id: Schema.optional(Identifier),
	status: Schema.Literals(["requested", "approved", "completed", "failed", "denied"]),
	tool_name: Schema.NonEmptyString,
});
export type CapabilityInvocationMetadata = typeof CapabilityInvocationMetadata.Type;

export const CapabilityRegistrySnapshot = Schema.Struct({
	capabilities: Schema.Array(CapabilitySummary),
	registry_version: Schema.Literal(1),
});
export type CapabilityRegistrySnapshot = typeof CapabilityRegistrySnapshot.Type;

export const MarketplaceEnableRequest = Schema.Struct({ id: Identifier, scope: MarketplaceScope });
export type MarketplaceEnableRequest = typeof MarketplaceEnableRequest.Type;
export const MarketplaceRemoveRequest = Schema.Struct({ id: Identifier, scope: MarketplaceScope });
export type MarketplaceRemoveRequest = typeof MarketplaceRemoveRequest.Type;
export const MarketplaceSyncRequest = Schema.Struct({
	engine_id: Identifier,
	id: Identifier,
	scope: MarketplaceScope,
});
export type MarketplaceSyncRequest = typeof MarketplaceSyncRequest.Type;
export const CapabilityHealthRequest = Schema.Struct({
	capability_id: Identifier,
	scope: MarketplaceScope,
});
export type CapabilityHealthRequest = typeof CapabilityHealthRequest.Type;
export const CapabilityLifecycleRequest = Schema.Struct({
	capability_id: Identifier,
	scope: MarketplaceScope,
});
export type CapabilityLifecycleRequest = typeof CapabilityLifecycleRequest.Type;

/** Stable ledger payload for Marketplace state transitions. */
export const MarketplaceLedgerEvent = Schema.Struct({
	approval_id: Schema.optional(Identifier),
	artifact_id: Schema.optional(Identifier),
	capability_health: Schema.optional(CapabilityHealth.fields.status),
	item_id: Identifier,
	item_kind: Schema.Literals(["routine", "capability"]),
	invocation_status: Schema.optional(
		Schema.Literals(["requested", "approved", "completed", "failed", "denied"]),
	),
	operation: Schema.Literals([
		"install_requested",
		"approval_resolved",
		"installed",
		"install_failed",
		"rolled_back",
		"enabled",
		"disabled",
		"removed",
		"synced",
		"drift_resolved",
		"connect_requested",
		"oauth_started",
		"oauth_completed",
		"oauth_refreshed",
		"oauth_revoked",
		"connected",
		"started",
		"reconnected",
		"health_checked",
		"disconnected",
		"restarted",
		"uninstalled",
		"invoked",
	]),
	status: MarketplaceItemStatus,
	sync_status: Schema.optional(ProviderSyncStatus),
	tool_name: Schema.optional(Schema.NonEmptyString),
	type: Schema.Literal("marketplace.lifecycle"),
});
export type MarketplaceLedgerEvent = typeof MarketplaceLedgerEvent.Type;
