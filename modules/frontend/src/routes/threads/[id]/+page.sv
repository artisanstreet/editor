<script lang="ts">
	import { page } from "$app/state";
	import ThreadRoute from "./thread-route.sv";

	const thread_id = $derived(page.params.id);
</script>

<svelte:head><title>Thread · Artisan Editor</title></svelte:head>

{#key thread_id}
	<ThreadRoute {thread_id} />
{/key}
