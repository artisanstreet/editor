import { join } from "node:path";

import type { DesktopPaths } from "./contracts";

export type ResolvedDesktopPaths = DesktopPaths;

/** Resolves all mutable and packaged locations explicitly rather than from CWD. */
export const resolve_desktop_paths = (input: {
	readonly app_data_path: string;
	readonly app_root_path: string;
	readonly is_packaged?: boolean;
	readonly resources_path: string;
}): ResolvedDesktopPaths => {
	const data_root = join(input.app_data_path, "Artisan Editor");
	/** electron-builder places app payloads below app.getAppPath() (app.asar), not resources/. */
	const packaged_root = input.app_root_path;

	const forge_root = input.is_packaged
		? join(input.resources_path, "artisan-forge")
		: join(input.app_root_path, ".dist", "forge");

	return {
		database_path: join(data_root, "artisan.sqlite"),
		forge_entry_path: join(forge_root, "host.js"),
		forge_executable_path: join(forge_root, "Artisan Forge.exe"),
		forge_native_runtime_path: join(forge_root, "native-runtime"),
		forge_node_executable_path: join(forge_root, "node.exe"),
		preload_path: input.is_packaged
			? join(packaged_root, "preload.cjs")
			: join(packaged_root, ".dist", "desktop", "preload.cjs"),
	};
};
