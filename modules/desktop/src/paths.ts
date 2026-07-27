import { isAbsolute, join } from "node:path";

export interface ResolvedDesktopPaths {
	readonly ae_command_path: string;
}

/** Resolves all mutable and packaged locations explicitly rather than from CWD. */
export const resolve_desktop_paths = (input: {
	readonly ae_command_override?: string;
	readonly is_packaged?: boolean;
	readonly resources_path: string;
}): ResolvedDesktopPaths => {
	const managed_ae =
		input.ae_command_override !== undefined && isAbsolute(input.ae_command_override)
			? input.ae_command_override
			: undefined;
	return {
		ae_command_path:
			managed_ae ??
			(input.is_packaged ? join(input.resources_path, "artisan-forge", "ae.cmd") : "ae"),
	};
};
