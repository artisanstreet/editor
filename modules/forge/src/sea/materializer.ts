import { Context, Crypto, Data, Effect, FileSystem, Layer, Path, Schema } from "effect";

import { SeaAssetSource } from "./asset-source";
import { SeaAssetManifest, type SeaAssetManifestEntry } from "./manifest";

const default_max_asset_bytes = 384 * 1024 * 1024;
const default_max_assets = 256;
const default_max_total_bytes = 512 * 1024 * 1024;

export class SeaAssetManifestInvalid extends Data.TaggedError("SeaAssetManifestInvalid")<{
	readonly cause: unknown;
}> {}

export class SeaAssetLimitExceeded extends Data.TaggedError("SeaAssetLimitExceeded")<{
	readonly asset_id: string;
	readonly limit: number;
	readonly observed: number;
	readonly scope: "asset" | "assets" | "total";
}> {}

export class SeaAssetIntegrityFailure extends Data.TaggedError("SeaAssetIntegrityFailure")<{
	readonly actual_byte_length: number;
	readonly actual_sha256: string;
	readonly asset_id: string;
	readonly expected_byte_length: number;
	readonly expected_sha256: string;
}> {}

export class SeaAssetMaterializationFailure extends Data.TaggedError(
	"SeaAssetMaterializationFailure",
)<{
	readonly asset_id: string;
	readonly cause: unknown;
}> {}

export interface SeaAssetMaterializerOptions {
	readonly cache_root: string;
	readonly max_asset_bytes?: number;
	readonly max_assets?: number;
	readonly max_total_bytes?: number;
}

export interface MaterializedSeaAsset {
	readonly asset_id: string;
	readonly executable: boolean;
	readonly path: string;
	readonly sha256: string;
}

export interface MaterializedSeaAssetSet {
	readonly assets: ReadonlyArray<MaterializedSeaAsset>;
	readonly root: string;
}

export interface SeaAssetMaterializerShape {
	readonly Materialize: (
		manifest: unknown,
	) => Effect.Effect<
		MaterializedSeaAssetSet,
		| SeaAssetManifestInvalid
		| SeaAssetLimitExceeded
		| SeaAssetIntegrityFailure
		| SeaAssetMaterializationFailure
		| import("./asset-source").SeaAssetUnavailable
	>;
}

export class SeaAssetMaterializer extends Context.Service<
	SeaAssetMaterializer,
	SeaAssetMaterializerShape
>()("Artisan/SeaAssetMaterializer") {}

const ToHex = (bytes: Uint8Array) => Buffer.from(bytes).toString("hex");
const ToBytes = (value: unknown) => new TextEncoder().encode(JSON.stringify(value));
const compare_asset_id = (left: SeaAssetManifestEntry, right: SeaAssetManifestEntry) =>
	left.asset_id.localeCompare(right.asset_id);

const Failure = (asset_id: string) => (cause: unknown) =>
	new SeaAssetMaterializationFailure({ asset_id, cause });

const VerifyFile = (
	file_system: FileSystem.FileSystem,
	crypto: Crypto.Crypto,
	entry: SeaAssetManifestEntry,
	asset_path: string,
) =>
	file_system.readFile(asset_path).pipe(
		Effect.flatMap((bytes) =>
			crypto.digest("SHA-256", bytes).pipe(
				Effect.map(ToHex),
				Effect.map(
					(sha256) => bytes.byteLength === entry.byte_length && sha256 === entry.sha256,
				),
			),
		),
		Effect.mapError(Failure(entry.asset_id)),
	);

const VerifyAssetSet = (
	file_system: FileSystem.FileSystem,
	crypto: Crypto.Crypto,
	entries: ReadonlyArray<SeaAssetManifestEntry>,
	assets: ReadonlyArray<MaterializedSeaAsset>,
) =>
	Effect.forEach(entries, (entry) => {
		const asset = assets.find((candidate) => candidate.asset_id === entry.asset_id);
		return asset === undefined
			? Effect.succeed(false)
			: VerifyFile(file_system, crypto, entry, asset.path).pipe(
					Effect.catchTag("SeaAssetMaterializationFailure", () => Effect.succeed(false)),
				);
	}).pipe(Effect.map((results) => results.every(Boolean)));

/**
 * Provides bounded, integrity-checked SEA extraction. Every package file is
 * placed beneath one manifest digest so node-pty, Koffi, and process hosts
 * keep their package-relative native loading layout.
 */
export const make_sea_asset_materializer_layer = (options: SeaAssetMaterializerOptions) => {
	const limits = {
		max_asset_bytes: options.max_asset_bytes ?? default_max_asset_bytes,
		max_assets: options.max_assets ?? default_max_assets,
		max_total_bytes: options.max_total_bytes ?? default_max_total_bytes,
	};
	return Layer.effect(
		SeaAssetMaterializer,
		Effect.gen(function* () {
			const source = yield* SeaAssetSource;
			const file_system = yield* FileSystem.FileSystem;
			const path = yield* Path.Path;
			const crypto = yield* Crypto.Crypto;
			return SeaAssetMaterializer.of({
				Materialize: (unknown_manifest) =>
					Effect.gen(function* () {
						const manifest = yield* Schema.decodeUnknownEffect(SeaAssetManifest)(
							unknown_manifest,
						).pipe(Effect.mapError((cause) => new SeaAssetManifestInvalid({ cause })));
						const entries = [...manifest.assets].sort(compare_asset_id);
						const unique_assets = new Set(entries.map((entry) => entry.asset_id));
						const unique_paths = new Set(entries.map((entry) => entry.relative_path));
						if (
							unique_assets.size !== entries.length ||
							unique_paths.size !== entries.length
						) {
							return yield* new SeaAssetManifestInvalid({
								cause: "SEA asset ids and relative paths must be unique",
							});
						}
						if (entries.length > limits.max_assets) {
							return yield* new SeaAssetLimitExceeded({
								asset_id: "manifest",
								limit: limits.max_assets,
								observed: entries.length,
								scope: "assets",
							});
						}
						const declared_total = entries.reduce(
							(sum, asset) => sum + asset.byte_length,
							0,
						);
						if (declared_total > limits.max_total_bytes) {
							return yield* new SeaAssetLimitExceeded({
								asset_id: "manifest",
								limit: limits.max_total_bytes,
								observed: declared_total,
								scope: "total",
							});
						}
						for (const entry of entries) {
							if (entry.byte_length > limits.max_asset_bytes) {
								return yield* new SeaAssetLimitExceeded({
									asset_id: entry.asset_id,
									limit: limits.max_asset_bytes,
									observed: entry.byte_length,
									scope: "asset",
								});
							}
						}
						const manifest_sha256 = yield* crypto
							.digest(
								"SHA-256",
								ToBytes({ assets: entries, version: manifest.version }),
							)
							.pipe(Effect.map(ToHex), Effect.mapError(Failure("manifest")));
						const root = path.join(options.cache_root, manifest_sha256);
						const assets = entries.map((entry) => ({
							asset_id: entry.asset_id,
							executable: entry.executable,
							path: path.join(root, ...entry.relative_path.split("/")),
							sha256: entry.sha256,
						}));
						const existing = yield* file_system
							.exists(root)
							.pipe(Effect.mapError(Failure("manifest")));
						if (existing) {
							const valid = yield* VerifyAssetSet(
								file_system,
								crypto,
								entries,
								assets,
							);
							if (valid) return { assets, root };
							return yield* new SeaAssetMaterializationFailure({
								asset_id: "manifest",
								cause: "The content-addressed SEA runtime cache failed verification",
							});
						}

						const random = yield* crypto
							.randomBytes(12)
							.pipe(Effect.map(ToHex), Effect.mapError(Failure("manifest")));
						const staging_root = `${root}.${random}.tmp`;
						yield* Effect.gen(function* () {
							let actual_total = 0;
							yield* file_system
								.makeDirectory(staging_root, { mode: 0o700, recursive: true })
								.pipe(Effect.mapError(Failure("manifest")));
							for (const entry of entries) {
								const bytes = yield* source.Read(entry.asset_id);
								actual_total += bytes.byteLength;
								if (
									bytes.byteLength > limits.max_asset_bytes ||
									actual_total > limits.max_total_bytes
								) {
									return yield* new SeaAssetLimitExceeded({
										asset_id: entry.asset_id,
										limit:
											bytes.byteLength > limits.max_asset_bytes
												? limits.max_asset_bytes
												: limits.max_total_bytes,
										observed:
											bytes.byteLength > limits.max_asset_bytes
												? bytes.byteLength
												: actual_total,
										scope:
											bytes.byteLength > limits.max_asset_bytes
												? "asset"
												: "total",
									});
								}
								const actual_sha256 = yield* crypto
									.digest("SHA-256", bytes)
									.pipe(
										Effect.map(ToHex),
										Effect.mapError(Failure(entry.asset_id)),
									);
								if (
									bytes.byteLength !== entry.byte_length ||
									actual_sha256 !== entry.sha256
								) {
									return yield* new SeaAssetIntegrityFailure({
										actual_byte_length: bytes.byteLength,
										actual_sha256,
										asset_id: entry.asset_id,
										expected_byte_length: entry.byte_length,
										expected_sha256: entry.sha256,
									});
								}
								const staged_path = path.join(
									staging_root,
									...entry.relative_path.split("/"),
								);
								const directory = path.dirname(staged_path);
								yield* file_system
									.makeDirectory(directory, { mode: 0o700, recursive: true })
									.pipe(Effect.mapError(Failure(entry.asset_id)));
								yield* file_system
									.writeFile(staged_path, bytes, {
										flag: "wx",
										mode: entry.executable ? 0o700 : 0o600,
									})
									.pipe(Effect.mapError(Failure(entry.asset_id)));
								if (entry.executable && process.platform !== "win32") {
									yield* file_system
										.chmod(staged_path, 0o700)
										.pipe(Effect.mapError(Failure(entry.asset_id)));
								}
							}
							yield* file_system
								.rename(staging_root, root)
								.pipe(
									Effect.catch((cause) =>
										VerifyAssetSet(file_system, crypto, entries, assets).pipe(
											Effect.flatMap((valid) =>
												valid
													? Effect.void
													: Effect.fail(Failure("manifest")(cause)),
											),
										),
									),
								);
						}).pipe(
							Effect.ensuring(
								file_system
									.remove(staging_root, { force: true, recursive: true })
									.pipe(Effect.ignore),
							),
						);
						return { assets, root };
					}),
			});
		}),
	);
};
