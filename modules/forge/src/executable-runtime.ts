import { tmpdir } from "node:os";
import { resolve } from "node:path";

export const ForgeSeaSmokeModeArgument = "--artisan-internal-sea-smoke";
export const ForgeSeaRuntimeSmokeModeArgument = "--artisan-internal-sea-runtime-smoke";

export const ResolveForgeSeaCacheRoot = (
	environment: NodeJS.ProcessEnv,
	temporary_directory = tmpdir(),
) => {
	const configured = environment.ARTISAN_SEA_CACHE_ROOT?.trim();
	if (configured !== undefined && configured.length > 0) return resolve(configured);

	const base = environment.LOCALAPPDATA ?? environment.XDG_CACHE_HOME ?? temporary_directory;

	return resolve(base, "Artisan", "Forge", "runtime");
};
