<script lang="ts">
	/**
	 * A project entered by name, with no thread open in it yet.
	 *
	 * The same new-thread surface the application opens on; the URL is simply
	 * what answers the project question here, which is why changing that word
	 * moves the URL with it.
	 */
	import { page } from "$app/state";
	import NewThreadRoute from "../../components/new-thread-route.svelte";

	const workspace_id = $derived(page.params.workspace ?? "");
</script>

{#key workspace_id}
	<NewThreadRoute {workspace_id} />
{/key}
