<script lang="ts">
	import { page } from "$app/state";
	import ThreadRoute from "../../../components/thread-route.sv";

	const thread_id = $derived(page.params.thread);
</script>

<svelte:head><title>Thread · Artisan Editor</title></svelte:head>

{#key `${page.params.workspace}:${thread_id}`}
	<ThreadRoute {thread_id} />
{/key}
