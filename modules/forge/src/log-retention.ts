import { stat, truncate } from "node:fs/promises";

import { Effect } from "effect";

export const MAX_FORGE_LOG_BYTES = 4 * 1_024 * 1_024;
export const FORGE_LOG_CHECK_INTERVAL = "1 second";

/**
 * Truncates the active detached log through the same narrow Node boundary that
 * owns the inherited stdout/stderr descriptor. Missing files are expected.
 */
export const TruncateForgeLogIfOversized = (path: string) =>
	Effect.tryPromise({
		try: async () => {
			const info = await stat(path).catch((cause: NodeJS.ErrnoException) =>
				cause.code === "ENOENT" ? undefined : Promise.reject(cause),
			);
			if (info !== undefined && info.size >= MAX_FORGE_LOG_BYTES) {
				await truncate(path, 0);
			}
		},
		catch: () => undefined,
	}).pipe(Effect.ignore);

/** Keeps a noisy long-running Forge process from growing its detached log forever. */
export const MaintainBoundedForgeLog = (path: string) =>
	Effect.forever(
		Effect.sleep(FORGE_LOG_CHECK_INTERVAL).pipe(
			Effect.andThen(TruncateForgeLogIfOversized(path)),
		),
	);
