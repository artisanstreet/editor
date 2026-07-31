<script lang="ts">
	import { page } from "$app/state";
	import ThreadRoute from "../../components/thread-route.sv";

	const thread_id = $derived(page.params.id);
</script>

<svelte:head><title>Opening thread · Artisan Editor</title></svelte:head>

{#key thread_id}
	<ThreadRoute {thread_id} />
{/key}
