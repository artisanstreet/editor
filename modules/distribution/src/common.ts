import { Schema } from "effect";

export const DistributionManifestFormatVersion = Schema.Literal(1);
export type DistributionManifestFormatVersion = typeof DistributionManifestFormatVersion.Type;

export const ReleaseChannel = Schema.Literals(["stable", "beta", "nightly"]);
export type ReleaseChannel = typeof ReleaseChannel.Type;

export const Platform = Schema.Literals(["windows", "macos", "linux"]);
export type Platform = typeof Platform.Type;

export const Architecture = Schema.Literals(["x64", "arm64"]);
export type Architecture = typeof Architecture.Type;

export const Libc = Schema.Literal("glibc");
export type Libc = typeof Libc.Type;

export const SemanticVersion = Schema.String.check(
	Schema.isPattern(
		/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/,
	),
);
export type SemanticVersion = typeof SemanticVersion.Type;

export const Sha256Digest = Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/));
export type Sha256Digest = typeof Sha256Digest.Type;

export const IsoTimestamp = Schema.String.check(
	Schema.isPattern(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/),
);
export type IsoTimestamp = typeof IsoTimestamp.Type;

export const AbsolutePath = Schema.String.check(
	Schema.isPattern(/^(?:[A-Za-z]:[\\/]|\/).+/, {
		message: "Expected an absolute filesystem path",
	}),
);
export type AbsolutePath = typeof AbsolutePath.Type;

export const MaximumArchivePathLength = 1_024;

export const SafeArchivePath = Schema.String.check(
	Schema.isMaxLength(MaximumArchivePathLength),
	Schema.isPattern(/^(?![\\/])(?![A-Za-z]:)(?!.*(?:^|[\\/])\.\.(?:[\\/]|$)).+$/, {
		message: "Expected a bounded relative archive path without parent traversal",
	}),
);
export type SafeArchivePath = typeof SafeArchivePath.Type;

export const TargetPlatform = Schema.Struct({
	platform: Platform,
	architecture: Architecture,
	libc: Schema.optional(Libc),
});
export type TargetPlatform = typeof TargetPlatform.Type;
