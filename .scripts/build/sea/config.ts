import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

export interface SeaBuildAssetInput {
	readonly asset_id: string;
	readonly executable?: boolean;
	readonly relative_path: string;
	readonly source_path: string;
}

export interface SeaBuildInput {
	readonly assets: ReadonlyArray<SeaBuildAssetInput>;
	readonly main_path: string;
	readonly output_path: string;
}

export interface SeaAssetManifestEntry {
	readonly asset_id: string;
	readonly byte_length: number;
	readonly executable: boolean;
	readonly relative_path: string;
	readonly sha256: string;
}

export interface SeaAssetManifest {
	readonly assets: ReadonlyArray<SeaAssetManifestEntry>;
	readonly version: 1;
}

export interface SeaBuildArtifacts {
	readonly asset_manifest: SeaAssetManifest;
	readonly asset_manifest_json: string;
	readonly sea_config: Readonly<{
		readonly assets: Readonly<Record<string, string>>;
		readonly disableExperimentalSEAWarning: true;
		readonly main: string;
		readonly output: string;
		readonly useCodeCache: false;
		readonly useSnapshot: false;
	}>;
	readonly sea_config_json: string;
}

/** SEA cache paths are deliberately portable, relative POSIX paths. */
const safe_relative_path = /^(?:[A-Za-z0-9@][A-Za-z0-9@._-]*)(?:\/[A-Za-z0-9@][A-Za-z0-9@._-]*)*$/;
const safe_asset_id = /^[A-Za-z][A-Za-z0-9._-]{0,127}$/;

const CanonicalJson = (value: unknown) => `${JSON.stringify(value, undefined, "\t")}\n`;

const Sha256 = (bytes: Uint8Array) => createHash("sha256").update(bytes).digest("hex");

const compare_asset_id = (left: SeaBuildAssetInput, right: SeaBuildAssetInput) =>
	left.asset_id.localeCompare(right.asset_id);

/**
 * Produces the two deterministic inputs to the Node 24 SEA injection step.
 * The injected main is an already-bundled CommonJS launcher; native and optional
 * process-host payloads remain explicit SEA assets so the runtime can verify
 * and materialize them after process startup.
 */
export const GenerateSeaBuildArtifacts = (
	input: SeaBuildInput,
	read_bytes: (source_path: string) => Uint8Array = readFileSync,
): SeaBuildArtifacts => {
	const assets = [...input.assets].sort(compare_asset_id);
	const asset_paths: Record<string, string> = {};
	const manifest_assets: Array<SeaAssetManifestEntry> = [];
	const seen_asset_ids = new Set<string>();
	const seen_relative_paths = new Set<string>();

	for (const asset of assets) {
		if (!safe_asset_id.test(asset.asset_id)) {
			throw new Error(`SEA asset id is not safe: ${asset.asset_id}`);
		}
		if (!safe_relative_path.test(asset.relative_path)) {
			throw new Error(`SEA asset path is not a safe relative path: ${asset.relative_path}`);
		}
		if (seen_asset_ids.has(asset.asset_id) || seen_relative_paths.has(asset.relative_path)) {
			throw new Error(`SEA asset ids and relative paths must be unique: ${asset.asset_id}`);
		}

		const bytes = read_bytes(asset.source_path);
		seen_asset_ids.add(asset.asset_id);
		seen_relative_paths.add(asset.relative_path);
		asset_paths[asset.asset_id] = asset.source_path;
		manifest_assets.push({
			asset_id: asset.asset_id,
			byte_length: bytes.byteLength,
			executable: asset.executable ?? false,
			relative_path: asset.relative_path,
			sha256: Sha256(bytes),
		});
	}

	const asset_manifest: SeaAssetManifest = { assets: manifest_assets, version: 1 };
	const sea_config = {
		assets: asset_paths,
		disableExperimentalSEAWarning: true as const,
		main: input.main_path,
		output: input.output_path,
		useCodeCache: false as const,
		useSnapshot: false as const,
	};

	return {
		asset_manifest,
		asset_manifest_json: CanonicalJson(asset_manifest),
		sea_config,
		sea_config_json: CanonicalJson(sea_config),
	};
};
