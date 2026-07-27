import { access } from "node:fs/promises";

import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { BuildWindowsDistributionReleaseFromEnvironment } from "../../../.scripts/package/build-distribution-release";

const RunPackagedReleaseBuild = process.env.ARTISAN_BUILD_PACKAGED_RELEASE_GATE === "1";

describe("packaged Windows distribution release build", () => {
	it.skipIf(!RunPackagedReleaseBuild)(
		"builds the signed distribution artifacts from the release environment",
		async () => {
			const output = await Effect.runPromise(
				BuildWindowsDistributionReleaseFromEnvironment(process.env),
			);

			await Promise.all([
				access(output.archive_path),
				access(output.manifest_path),
				access(output.signature_path),
			]);

			expect(output.archive_entries.length).toBeGreaterThan(0);
		},
		120_000,
	);
});
