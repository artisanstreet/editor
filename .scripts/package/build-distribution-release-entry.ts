import { basename } from "node:path";

import { Effect } from "effect";

import { BuildWindowsDistributionReleaseFromEnvironment } from "./build-distribution-release.ts";

Effect.runPromise(BuildWindowsDistributionReleaseFromEnvironment(process.env))
	.then((output) => {
		process.stdout.write(
			`${JSON.stringify({
				archive: basename(output.archive_path),
				entries: output.archive_entries.length,
				manifest: basename(output.manifest_path),
				signature: basename(output.signature_path),
			})}\n`,
		);
	})
	.catch((cause: unknown) => {
		process.stderr.write(
			`${cause instanceof Error ? cause.message : "Distribution release build failed"}\n`,
		);
		process.exitCode = 1;
	});
