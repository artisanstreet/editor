import { Schema } from "effect";

import { Identifier, IsoDateTime, PositiveInt } from "./common";

/** Identifies an immutable rich-link binary asset by its lowercase SHA-256 digest. */
export const PreviewAssetId = Schema.String.check(
	Schema.isPattern(/^[a-f0-9]{64}$/, {
		message: "Expected a lowercase SHA-256 asset identifier",
	}),
);

export type PreviewAssetId = typeof PreviewAssetId.Type;

const PreviewUrl = Schema.String.check(
	Schema.makeFilter<string>((value) => {
		try {
			const url = new URL(value);

			return (url.protocol === "http:" || url.protocol === "https:") &&
				url.username.length === 0 &&
				url.password.length === 0 &&
				url.hash.length === 0
				? undefined
				: "Expected a credential-free HTTP(S) URL without a fragment";
		} catch {
			return "Expected a valid credential-free HTTP(S) URL without a fragment";
		}
	}),
);

/** Restricts preview-server registrations to loopback HTTP(S) origins. */
export const LocalPreviewUrl = PreviewUrl.check(
	Schema.makeFilter<string>((value) => {
		const hostname = new URL(value).hostname.toLowerCase();

		return hostname === "localhost" ||
			hostname.endsWith(".localhost") ||
			hostname === "127.0.0.1" ||
			hostname === "::1"
			? undefined
			: "Expected an explicit localhost or loopback preview URL";
	}),
);

export type LocalPreviewUrl = typeof LocalPreviewUrl.Type;

/** Carries one normalized route advertised by a local preview server. */
export const PreviewRoute = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.startsWith("/") && value.length <= 4_096 && !/[\p{Cc}]/u.test(value)
			? undefined
			: "Expected a bounded control-character-free preview route beginning with /",
	),
);

export type PreviewRoute = typeof PreviewRoute.Type;

const PreviewPort = Schema.Int.check(
	Schema.isGreaterThanOrEqualTo(1),
	Schema.isLessThanOrEqualTo(65_535),
);

/** Records the process-like local owner associated with a preview target. */
export const PreviewTargetSource = Schema.Union([
	Schema.Struct({ kind: Schema.Literal("process"), process_id: Identifier }),
	Schema.Struct({ kind: Schema.Literal("terminal"), terminal_id: Identifier }),
]);

export type PreviewTargetSource = typeof PreviewTargetSource.Type;

export const PreviewTargetState = Schema.Literals([
	"healthy",
	"removed",
	"registered",
	"stopped",
	"unhealthy",
]);

export type PreviewTargetState = typeof PreviewTargetState.Type;

/** Captures the durable external-browser launch condition independently from target health. */
export const PreviewLaunchState = Schema.Literals([
	"idle",
	"launching",
	"launched",
	"unavailable",
	"error",
]);
export type PreviewLaunchState = typeof PreviewLaunchState.Type;

const PreviewLastError = Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(1024));

/** Projects a bounded, content-free local health observation. */
export const PreviewTargetHealth = Schema.Struct({
	checked_at: IsoDateTime,
	latency_ms: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	message: Schema.optional(Schema.String.check(Schema.isMaxLength(1024))),
	status: Schema.Literals(["healthy", "unhealthy"]),
	status_code: Schema.optional(
		Schema.Int.check(Schema.isGreaterThanOrEqualTo(100), Schema.isLessThanOrEqualTo(599)),
	),
});

export type PreviewTargetHealth = typeof PreviewTargetHealth.Type;

/** Exposes one explicit local preview target without embedding or fetching its page. */
export const PreviewTarget = Schema.Struct({
	created_at: IsoDateTime,
	health: Schema.optional(PreviewTargetHealth),
	id: Identifier,
	journal_sequence: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	last_error: Schema.optional(PreviewLastError),
	launch_state: PreviewLaunchState,
	port: PreviewPort,
	project_id: Identifier,
	routes: Schema.Array(PreviewRoute).check(Schema.isMaxLength(512)),
	source: Schema.optional(PreviewTargetSource),
	state: PreviewTargetState,
	thread_id: Identifier,
	updated_at: IsoDateTime,
	url: LocalPreviewUrl,
	workspace_id: Identifier,
});

export type PreviewTarget = typeof PreviewTarget.Type;

/** Emits a durable target projection update for loading, unavailable, health, and reconnecting UI state. */
export const PreviewTargetUpdatedEvent = Schema.Struct({
	target: PreviewTarget,
	type: Schema.Literal("preview.target.updated"),
});
export type PreviewTargetUpdatedEvent = typeof PreviewTargetUpdatedEvent.Type;

export const PreviewTargetRegistration = Schema.Struct({
	id: Identifier,
	port: PreviewPort,
	project_id: Identifier,
	routes: Schema.Array(PreviewRoute).check(Schema.isMaxLength(512)),
	source: Schema.optional(PreviewTargetSource),
	thread_id: Identifier,
	url: LocalPreviewUrl,
	workspace_id: Identifier,
});

export type PreviewTargetRegistration = typeof PreviewTargetRegistration.Type;

export const PreviewTargetListQuery = Schema.Struct({ workspace_id: Schema.optional(Identifier) });
export type PreviewTargetListQuery = typeof PreviewTargetListQuery.Type;

export const PreviewTargetGetQuery = Schema.Struct({ target_id: Identifier });
export type PreviewTargetGetQuery = typeof PreviewTargetGetQuery.Type;

export const PreviewTargetStateRequest = Schema.Struct({
	state: PreviewTargetState,
	target_id: Identifier,
});
export type PreviewTargetStateRequest = typeof PreviewTargetStateRequest.Type;

export const PreviewTargetRemoveRequest = Schema.Struct({ target_id: Identifier });
export type PreviewTargetRemoveRequest = typeof PreviewTargetRemoveRequest.Type;

/** Resolves public rich-link metadata; the backend enforces network policy and never executes page scripts. */
export const RichLinkResolveQuery = Schema.Struct({ url: PreviewUrl });
export type RichLinkResolveQuery = typeof RichLinkResolveQuery.Type;

export const RichLinkAssetMetadata = Schema.Struct({
	asset_id: PreviewAssetId,
	bytes: PositiveInt,
	content_type: Schema.String.check(Schema.isMaxLength(256)),
});
export type RichLinkAssetMetadata = typeof RichLinkAssetMetadata.Type;

export const RichLinkFavicon = Schema.Struct({
	...RichLinkAssetMetadata.fields,
	source: Schema.Literals(["apple_touch", "document_icon", "fallback"]),
	source_url: PreviewUrl,
});
export type RichLinkFavicon = typeof RichLinkFavicon.Type;

export const RichLinkResolution = Schema.Struct({
	cache: Schema.Struct({ expires_at: IsoDateTime, status: Schema.Literals(["hit", "miss"]) }),
	favicon: Schema.optional(RichLinkFavicon),
	fetched_at: IsoDateTime,
	final_url: PreviewUrl,
	page_name: Schema.String.check(Schema.isMaxLength(512)),
	requested_url: PreviewUrl,
	site_name: Schema.String.check(Schema.isMaxLength(512)),
	title: Schema.optional(Schema.String.check(Schema.isMaxLength(512))),
});
export type RichLinkResolution = typeof RichLinkResolution.Type;

/** Looks up metadata for a retained binary asset; bytes travel only over asset:<sha256>. */
export const PreviewAssetMetadataQuery = Schema.Struct({ asset_id: PreviewAssetId });
export type PreviewAssetMetadataQuery = typeof PreviewAssetMetadataQuery.Type;

/** Launches a target in the configured external browser and never returns browser content. */
export const PreviewBrowserLaunchRequest = Schema.Struct({ target_id: Identifier });
export type PreviewBrowserLaunchRequest = typeof PreviewBrowserLaunchRequest.Type;

export const PreviewBrowserLaunch = Schema.Struct({
	launched_at: IsoDateTime,
	target_id: Identifier,
});
export type PreviewBrowserLaunch = typeof PreviewBrowserLaunch.Type;

/** Opens an attributable external-browser inspection session; it is distinct from browser launch. */
export const PreviewInspectionSessionOpenRequest = Schema.Struct({
	connector_id: Identifier,
	target_id: Identifier,
});
export type PreviewInspectionSessionOpenRequest = typeof PreviewInspectionSessionOpenRequest.Type;

export const PreviewInspectionSession = Schema.Struct({
	closed_at: Schema.optional(IsoDateTime),
	connector_id: Identifier,
	last_error: Schema.optional(PreviewLastError),
	opened_at: IsoDateTime,
	reconnect_state: Schema.Literals(["connected", "reconnecting", "unavailable", "error"]),
	session_id: Identifier,
	state: Schema.Literals(["open", "closed", "abandoned"]),
	target_id: Identifier,
	updated_at: IsoDateTime,
});
export type PreviewInspectionSession = typeof PreviewInspectionSession.Type;

/** Records inspection lifecycle changes so abandoned sessions can be shown and cleaned up after restart. */
export const PreviewInspectionSessionUpdatedEvent = Schema.Struct({
	session: PreviewInspectionSession,
	type: Schema.Literal("preview.inspection.updated"),
});
export type PreviewInspectionSessionUpdatedEvent = typeof PreviewInspectionSessionUpdatedEvent.Type;

/** Restricts inspection to an explicit V1 capability rather than arbitrary browser automation. */
export const PreviewInspectionRequest = Schema.Struct({
	operation: Schema.Literals(["health", "metadata"]),
	session_id: Identifier,
});
export type PreviewInspectionRequest = typeof PreviewInspectionRequest.Type;

/** Returns a small, transport-safe observation and never page DOM, cookies, or browser credentials. */
export const PreviewInspectionResult = Schema.Union([
	Schema.Struct({
		health: PreviewTargetHealth,
		operation: Schema.Literal("health"),
		session_id: Identifier,
	}),
	Schema.Struct({
		operation: Schema.Literal("metadata"),
		session_id: Identifier,
		target: PreviewTarget,
	}),
]);
export type PreviewInspectionResult = typeof PreviewInspectionResult.Type;

export const PreviewInspectionSessionCloseRequest = Schema.Struct({ session_id: Identifier });
export type PreviewInspectionSessionCloseRequest = typeof PreviewInspectionSessionCloseRequest.Type;
