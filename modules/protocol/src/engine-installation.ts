import { Schema } from "effect";

import { Identifier, IsoDateTime } from "./common";

/** The lifecycle position of one engine's managed install. */
export const EngineInstallationActivity = Schema.Literals([
	"authenticating",
	"failed",
	"idle",
	"installing",
]);
export type EngineInstallationActivity = typeof EngineInstallationActivity.Type;

/** The step an in-flight install is currently executing. */
export const EngineInstallationPhase = Schema.Literals([
	"checking",
	"downloading",
	"provisioning",
	"resolving",
	"staging",
	"verifying",
]);
export type EngineInstallationPhase = typeof EngineInstallationPhase.Type;

/**
 * Projects one engine's Artisan-owned installation. `managed` means an
 * Artisan-owned binary is installed and active; it never describes or falls
 * back to a system installation.
 */
export const EngineInstallationReport = Schema.Struct({
	active_version: Schema.optional(Schema.String.check(Schema.isMinLength(1))),
	activity: EngineInstallationActivity,
	activity_phase: Schema.optional(EngineInstallationPhase),
	/** Whether the owned config home already carries a provider sign-in. */
	credentials_present: Schema.Boolean,
	display_name: Schema.String.check(Schema.isMinLength(1)),
	engine_id: Schema.String.check(Schema.isMinLength(1)),
	failure: Schema.optional(Schema.String.check(Schema.isMinLength(1))),
	/** The vendor channel's newest version, present when updates were checked. */
	latest_version: Schema.optional(Schema.String.check(Schema.isMinLength(1))),
	managed: Schema.Boolean,
	minimum_version: Schema.optional(Schema.String.check(Schema.isMinLength(1))),
	previous_version: Schema.optional(Schema.String.check(Schema.isMinLength(1))),
	/** The version Artisan has verified against this build's adapters. */
	recommended_version: Schema.optional(Schema.String.check(Schema.isMinLength(1))),
	/**
	 * Whether the active version is behind Artisan's update target: the tested
	 * recommended version when available, otherwise the vendor's latest version.
	 * `latest_version` remains informational when a recommendation exists.
	 */
	update_available: Schema.optional(Schema.Boolean),
});
export type EngineInstallationReport = typeof EngineInstallationReport.Type;

/**
 * Requests managed-install state. `check_updates` additionally consults the
 * vendor release channel (cached backend-side) so the report can carry
 * informational `latest_version`. Installation state is service-scoped and
 * authoritative for this Forge lifecycle, so callers poll rather than subscribe.
 */
export const EngineInstallationQuery = Schema.Struct({
	check_updates: Schema.optional(Schema.Boolean),
	engine_id: Schema.optional(Identifier),
});
export type EngineInstallationQuery = typeof EngineInstallationQuery.Type;

/** Carries the managed-install reports for the queried engines. */
export const EngineInstallationSnapshot = Schema.Struct({
	engines: Schema.Array(EngineInstallationReport).check(Schema.isMaxLength(16)),
	fetched_at: IsoDateTime,
});
export type EngineInstallationSnapshot = typeof EngineInstallationSnapshot.Type;

/**
 * Starts a managed install. Without `version` the backend installs the
 * recommended (tested) version, falling back to the vendor's stable channel.
 * The work runs in the background: an accepted result means the install
 * started, and progress is observed by re-issuing the installation query.
 */
export const EngineInstallRequest = Schema.Struct({
	engine_id: Identifier,
	version: Schema.optional(Schema.String.check(Schema.isMinLength(1))),
});
export type EngineInstallRequest = typeof EngineInstallRequest.Type;

/** Starts the provider sign-in flow in Artisan's owned config home. */
export const EngineAuthenticationRequest = Schema.Struct({
	engine_id: Identifier,
});
export type EngineAuthenticationRequest = typeof EngineAuthenticationRequest.Type;

/** Restores the previously active managed version. */
export const EngineRollbackRequest = Schema.Struct({
	engine_id: Identifier,
});
export type EngineRollbackRequest = typeof EngineRollbackRequest.Type;

/**
 * Settles an installation mutation as accepted or refused, never silently.
 * Acceptance confirms the operation was admitted and the accompanying report
 * is current: installs and authentication may still be running, while a
 * rollback may already be complete.
 */
export const EngineInstallationMutationResult = Schema.Union([
	Schema.Struct({
		report: EngineInstallationReport,
		status: Schema.Literal("accepted"),
	}),
	Schema.Struct({
		message: Schema.String.check(Schema.isMinLength(1)),
		status: Schema.Literal("rejected"),
	}),
]);
export type EngineInstallationMutationResult = typeof EngineInstallationMutationResult.Type;
