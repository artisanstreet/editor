	<script lang="ts">
	import { page } from "$app/state";
	import ThreadRouteGate from "../../../components/thread-route-gate.svelte";

	const thread_id = $derived(page.params.thread);
</script>

{#key `${page.params.workspace}:${thread_id}`}
	<ThreadRouteGate {thread_id} />
{/key}
