/** Renderer-safe OS identity projection, never a capability to inspect the host. */
export interface DesktopIdentity {
	readonly avatar_data_url?: string;
	readonly avatar_seed: string;
	readonly display_name: string;
	readonly machine_name: string;
}

export interface DesktopPaths {
	readonly database_path: string;
	readonly forge_entry_path: string;
	readonly forge_executable_path: string;
	readonly forge_native_runtime_path: string;
	readonly forge_node_executable_path: string;
	readonly preload_path: string;
}
