import { createHash } from "node:crypto";
import { readdir, readFile } from "node:fs/promises";
import { basename, join } from "node:path";

export type FrozenArtifact = {
	readonly name: string;
	readonly size: number;
	readonly sha256: string;
};

export type FrozenArtifactManifest = {
	readonly schema_version: 1;
	readonly version: string;
	readonly commit: string;
	readonly run_id: string;
	readonly artifacts: ReadonlyArray<FrozenArtifact>;
};

export const inspect_artifacts = async (
	directory: string,
): Promise<ReadonlyArray<FrozenArtifact>> => {
	const names = (await readdir(directory)).sort();
	return Promise.all(
		names.map(async (name) => {
			const bytes = await readFile(join(directory, name));
			return {
				name: basename(name),
				size: bytes.byteLength,
				sha256: createHash("sha256").update(bytes).digest("hex"),
			};
		}),
	);
};

export const create_artifact_manifest = async (
	directory: string,
	version: string,
	commit: string,
	run_id: string,
): Promise<FrozenArtifactManifest> => ({
	schema_version: 1,
	version,
	commit,
	run_id,
	artifacts: await inspect_artifacts(directory),
});

export const verify_artifact_manifest = async (
	directory: string,
	manifest: FrozenArtifactManifest,
	version: string,
	commit: string,
	run_id: string,
): Promise<void> => {
	if (
		manifest.schema_version !== 1 ||
		manifest.version !== version ||
		manifest.commit !== commit ||
		manifest.run_id !== run_id
	) {
		throw new Error("Artifact manifest identity does not match the selected candidate.");
	}
	const actual = await inspect_artifacts(directory);
	if (JSON.stringify(actual) !== JSON.stringify(manifest.artifacts)) {
		throw new Error("Artifact manifest does not match the ordered candidate bytes.");
	}
};
