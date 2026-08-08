import { Schema } from "effect";

export const SeaSafeAssetId = Schema.String.check(
	Schema.isPattern(/^[A-Za-z][A-Za-z0-9._-]{0,127}$/),
);

/**
 * Portable paths have no roots, separators from another platform, traversal,
 * or Windows drive syntax. The materializer joins only this validated form.
 */
export const SeaSafeRelativePath = Schema.String.check(
	Schema.isPattern(/^(?:[A-Za-z0-9@][A-Za-z0-9@._-]*)(?:\/[A-Za-z0-9@][A-Za-z0-9@._-]*)*$/),
);

export const SeaSha256 = Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/));

export const SeaAssetManifestEntry = Schema.Struct({
	asset_id: SeaSafeAssetId,
	byte_length: Schema.Int.check(Schema.isBetween({ maximum: 512 * 1024 * 1024, minimum: 0 })),
	executable: Schema.Boolean,
	relative_path: SeaSafeRelativePath,
	sha256: SeaSha256,
});

export const SeaAssetManifest = Schema.Struct({
	assets: Schema.Array(SeaAssetManifestEntry).check(Schema.isLengthBetween(0, 4_096)),
	version: Schema.Literal(1),
});

export type SeaAssetManifest = typeof SeaAssetManifest.Type;
export type SeaAssetManifestEntry = typeof SeaAssetManifestEntry.Type;
