import { readFile, writeFile } from "node:fs/promises";

import {
	create_artifact_manifest,
	verify_artifact_manifest,
	type FrozenArtifactManifest,
} from "./artifact-manifest.ts";

const [operation, directory, manifest_path, version, commit, run_id] = process.argv.slice(2);
if (!operation || !directory || !manifest_path || !version || !commit || !run_id) {
	throw new Error(
		"Usage: artifact-cli <create|verify> <directory> <manifest> <version> <commit> <run-id>",
	);
}
if (operation === "create") {
	const manifest = await create_artifact_manifest(directory, version, commit, run_id);
	await writeFile(manifest_path, `${JSON.stringify(manifest, undefined, "\t")}\n`);
} else if (operation === "verify") {
	const manifest = JSON.parse(await readFile(manifest_path, "utf8")) as FrozenArtifactManifest;
	await verify_artifact_manifest(directory, manifest, version, commit, run_id);
} else {
	throw new Error(`Unsupported operation ${operation}.`);
}
