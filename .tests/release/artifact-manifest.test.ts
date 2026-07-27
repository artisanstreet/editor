import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import {
	create_artifact_manifest,
	verify_artifact_manifest,
} from "../../build/release/artifact-manifest.ts";

describe("candidate artifact manifest", () => {
	it("binds ordered names, sizes, digests, and release identity", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-release-"));
		await writeFile(join(root, "b"), "two");
		await writeFile(join(root, "a"), "one");
		const manifest = await create_artifact_manifest(root, "0.1.0", "a".repeat(40), "7");
		expect(manifest.artifacts.map((artifact) => artifact.name)).toEqual(["a", "b"]);
		await expect(
			verify_artifact_manifest(root, manifest, "0.1.0", "a".repeat(40), "7"),
		).resolves.toBeUndefined();
		await writeFile(join(root, "a"), "changed");
		await expect(
			verify_artifact_manifest(root, manifest, "0.1.0", "a".repeat(40), "7"),
		).rejects.toThrow("ordered candidate bytes");
	});
});
