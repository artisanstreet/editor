<script lang="ts">
	import { goto } from "$app/navigation";
	import { page } from "$app/state";
	import { ThreadRoutePath } from "$lib/root/thread-navigation";
	import ThreadRoute from "./thread-route.sv";

	const thread_id = $derived(page.params.id);

	$effect(() => {
		const canonical_path = ThreadRoutePath(thread_id);
		if (page.url.pathname !== canonical_path) {
			void goto(canonical_path, {
				keepFocus: true,
				noScroll: true,
				replaceState: true,
			});
		}
	});
</script>

<svelte:head><title>Thread · Artisan Editor</title></svelte:head>

{#key thread_id}
	<ThreadRoute {thread_id} />
{/key}
