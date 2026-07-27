import { Schema } from "effect";

import {
	Architecture,
	DistributionManifestFormatVersion,
	Libc,
	Platform,
	ReleaseChannel,
	SafeArchivePath,
	SemanticVersion,
	Sha256Digest,
} from "./common";

export const ArtifactArchiveFormat = Schema.Literals(["zip", "tar.zst"]);
export type ArtifactArchiveFormat = typeof ArtifactArchiveFormat.Type;

/** Product artifacts are downloaded into bootstrap memory before verification. */
// The complete Windows x64 compatibility release currently compresses to
// roughly 587 MiB because it contains Electron, Forge's Node runtime, native
// terminal bindings, and the offline frontend. Keep a finite transport bound
// with enough headroom for that measured artifact.
export const MaximumArtifactBytes = 768 * 1024 * 1024;

/** Bounds both manifest decoding and the ZIP central-directory work set. */
export const MaximumArchiveEntries = 16_384;

/** Individual entries stream to disk but remain bounded against pathological payloads. */
export const MaximumArchiveEntryBytes = 1024 * 1024 * 1024;

/** Total uncompressed output is bounded independently from the compressed artifact. */
export const MaximumExpandedArtifactBytes = 2 * 1024 * 1024 * 1024;

export const ReleaseArtifact = Schema.Struct({
	artifact_id: Schema.NonEmptyString,
	platform: Platform,
	architecture: Architecture,
	libc: Schema.optional(Libc),
	archive_format: ArtifactArchiveFormat,
	file_name: SafeArchivePath,
	byte_size: Schema.Int.check(Schema.isBetween({ minimum: 1, maximum: MaximumArtifactBytes })),
	sha256: Sha256Digest,
	archive_entries: Schema.NonEmptyArray(SafeArchivePath).check(
		Schema.isMaxLength(MaximumArchiveEntries),
	),
});
export type ReleaseArtifact = typeof ReleaseArtifact.Type;

export const SigningIdentity = Schema.Struct({
	key_id: Schema.NonEmptyString,
	algorithm: Schema.Literal("ed25519"),
});
export type SigningIdentity = typeof SigningIdentity.Type;

export const ReleaseManifest = Schema.Struct({
	format_version: DistributionManifestFormatVersion,
	product_version: SemanticVersion,
	editor_forge_compatibility_version: SemanticVersion,
	channel: ReleaseChannel,
	signing_identity: SigningIdentity,
	minimum_bootstrap_version: SemanticVersion,
	minimum_cli_version: SemanticVersion,
	artifacts: Schema.NonEmptyArray(ReleaseArtifact).check(Schema.isMaxLength(64)),
});
export type ReleaseManifest = typeof ReleaseManifest.Type;
