import { join } from "node:path";

import type { DesktopPaths } from "./contracts";

/** Resolves all mutable and packaged locations explicitly rather than from CWD. */
export const resolve_desktop_paths = (input: {
	readonly app_data_path: string;
	readonly app_root_path: string;
	readonly resources_path: string;
}): DesktopPaths => {
	const data_root = join(input.app_data_path, "Artisan Editor");
	/** electron-builder places app payloads below app.getAppPath() (app.asar), not resources/. */
	const packaged_root = input.app_root_path;
	const source_root = input.app_root_path;

	const frontend_root = join(packaged_root, ".dist", "frontend");

	return {
		database_path: join(data_root, "artisan.sqlite"),
		frontend_index_path: join(frontend_root, "index.html"),
		frontend_root,
		migrations_path: join(source_root, "modules", "backend", "drizzle"),
		preload_path: join(packaged_root, ".dist", "desktop", "preload.cjs"),
		utility_path: join(packaged_root, ".dist", "desktop", "utility.js"),
	};
};
