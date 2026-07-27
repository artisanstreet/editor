import { basename } from "node:path";
import { inspect } from "node:util";

import { Effect } from "effect";

import {
	BuildWindowsDistributionReleaseFromEnvironment,
	DistributionReleaseBuildError,
} from "./build-distribution-release.ts";

const RenderFailure = (cause: unknown) => {
	if (cause instanceof DistributionReleaseBuildError)
		return `${cause.code}: ${
			cause.cause instanceof Error
				? (cause.cause.stack ?? cause.cause.message)
				: inspect(cause.cause)
		}`;
	return cause instanceof Error ? (cause.stack ?? cause.message) : inspect(cause);
};

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
		process.stderr.write(`${RenderFailure(cause)}\n`);
		process.exit(1);
	});
