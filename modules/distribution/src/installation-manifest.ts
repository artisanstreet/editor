import { Schema } from "effect";

import {
	AbsolutePath,
	Architecture,
	DistributionManifestFormatVersion,
	IsoTimestamp,
	Platform,
	ReleaseChannel,
	SemanticVersion,
	Sha256Digest,
} from "./common";

export const InstalledComponents = Schema.Struct({
	editor: Schema.Boolean,
	forge: Schema.Boolean,
});
export type InstalledComponents = typeof InstalledComponents.Type;

export const OwnedIntegration = Schema.Struct({
	path: Schema.NonEmptyString,
	fingerprint: Schema.NonEmptyString,
});
export type OwnedIntegration = typeof OwnedIntegration.Type;

export const InstalledIntegrations = Schema.Struct({
	ae_path: Schema.optional(OwnedIntegration),
	protocol: Schema.optional(OwnedIntegration),
	application_shortcut: Schema.optional(OwnedIntegration),
	forge_logs_shortcut: Schema.optional(OwnedIntegration),
	forge_start_shortcut: Schema.optional(OwnedIntegration),
	uninstall_shortcut: Schema.optional(OwnedIntegration),
	desktop_shortcut: Schema.optional(OwnedIntegration),
	autostart: Schema.optional(OwnedIntegration),
});
export type InstalledIntegrations = typeof InstalledIntegrations.Type;

export const InstalledArtifactIdentity = Schema.Struct({
	artifact_id: Schema.NonEmptyString,
	sha256: Sha256Digest,
	signing_key_id: Schema.NonEmptyString,
});
export type InstalledArtifactIdentity = typeof InstalledArtifactIdentity.Type;

export const PendingInstallationTransaction = Schema.Struct({
	state: Schema.Literals(["downloading", "verified", "staged", "integrating", "activating"]),
	target_version: SemanticVersion,
	staging_path: AbsolutePath,
	started_at: IsoTimestamp,
});
export type PendingInstallationTransaction = typeof PendingInstallationTransaction.Type;

export const RollbackPendingInstallationTransaction = Schema.Struct({
	state: Schema.Literal("rollback_pending"),
	target_version: SemanticVersion,
	restore_version: SemanticVersion,
	staging_path: AbsolutePath,
	started_at: IsoTimestamp,
});
export type RollbackPendingInstallationTransaction =
	typeof RollbackPendingInstallationTransaction.Type;

export const ActiveInstallationTransaction = Schema.Union([
	PendingInstallationTransaction,
	RollbackPendingInstallationTransaction,
]);
export type ActiveInstallationTransaction = typeof ActiveInstallationTransaction.Type;

export const InstallationTransaction = Schema.Union([
	Schema.Struct({ state: Schema.Literal("idle") }),
	ActiveInstallationTransaction,
]);
export type InstallationTransaction = typeof InstallationTransaction.Type;

const InstallationManifestCommon = {
	format_version: DistributionManifestFormatVersion,
	install_root: AbsolutePath,
	platform: Platform,
	architecture: Architecture,
	channel: ReleaseChannel,
	components: InstalledComponents,
	integrations: InstalledIntegrations,
	installed_at: IsoTimestamp,
	updated_at: IsoTimestamp,
} as const;

export const UnactivatedInstallationManifest = Schema.Struct({
	...InstallationManifestCommon,
	activation_state: Schema.Literal("unactivated"),
	transaction: PendingInstallationTransaction,
});
export type UnactivatedInstallationManifest = typeof UnactivatedInstallationManifest.Type;

export const ActivatedInstallationManifest = Schema.Struct({
	...InstallationManifestCommon,
	activation_state: Schema.Literal("active"),
	/**
	 * Absent only on legacy format-v1 manifests. Readers migrate absence to
	 * `pending`; newly written active manifests always persist an explicit value.
	 */
	finalization_state: Schema.optional(Schema.Literals(["pending", "complete"])),
	active_version: SemanticVersion,
	permanent_ae_path: AbsolutePath,
	previous_version: Schema.optional(SemanticVersion),
	artifact: InstalledArtifactIdentity,
	transaction: InstallationTransaction,
});
export type ActivatedInstallationManifest = typeof ActivatedInstallationManifest.Type;

export const InstallationManifest = Schema.Union([
	UnactivatedInstallationManifest,
	ActivatedInstallationManifest,
]);
export type InstallationManifest = typeof InstallationManifest.Type;
