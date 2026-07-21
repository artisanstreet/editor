import type { ArtisanToolPermissionPolicy, ThreadSessionPolicy } from "@artisan/protocol";

/**
 * Narrows built-in tool discovery to the selected thread's durable session policy.
 * A renderer without an authoritative policy fails closed for mutating capabilities.
 */
export const MakeSessionToolPolicy = (
	policy: ThreadSessionPolicy | undefined,
): ArtisanToolPermissionPolicy => {
	const workspace_write = policy?.sandbox_mode === "workspace_write";

	return {
		approval: policy?.permission_mode ?? "never",
		allow_engine_observation: true,
		allow_git_index_write: workspace_write,
		allow_preview_control: workspace_write,
		allow_process_control: workspace_write,
		allow_workspace_read: true,
		allow_workspace_write: workspace_write,
	};
};
