<script lang="ts">
	import { page } from "$app/state";
	import { EditorWorkspaceId } from "$lib/editor/workspace-identity";
	import EditorRoute from "../components/editor-route.sv";

	/** Compatibility entry point for pre-workspace/thread editor deep links. */
	const workspace_id = $derived(
		EditorWorkspaceId(page.url.searchParams.get("workspace") ?? undefined),
	);
</script>

<EditorRoute {workspace_id} />
