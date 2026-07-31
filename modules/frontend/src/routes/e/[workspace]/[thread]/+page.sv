<script lang="ts">
	import { page } from "$app/state";
	import EditorRouteGate from "./editor-route-gate.sv";

	const workspace_id = $derived(page.params.workspace);
	const thread_id = $derived(page.params.thread);
</script>

<svelte:head><title>Editor · Artisan Editor</title></svelte:head>

{#key `${workspace_id}:${thread_id}`}
	<EditorRouteGate {workspace_id} {thread_id} />
{/key}
