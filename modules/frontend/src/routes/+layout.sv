<script lang="ts">
	import "$lib/styles/fonts.css";
	import "$lib/styles/global.css";
	import "$lib/styles/artisan-compatibility.css";

	import { page } from "$app/state";
	import { ModeWatcher } from "mode-watcher";
	import ArtisanSidebar from "./components/artisan-sidebar.sv";
	import SectionedPanel from "./components/sectioned-panel.sv";
	import ThreadPanel from "./components/thread-panel.sv";

	let { children } = $props();
	const is_thread = $derived(/^\/thread\/[^/]+\/?$/.test(page.url.pathname));
</script>

<ModeWatcher defaultMode="dark" />

{#snippet sidebar()}
	<ArtisanSidebar />
{/snippet}

{#snippet primary()}
	{@render children()}
{/snippet}

{#snippet secondary()}
	<ThreadPanel />
{/snippet}

<SectionedPanel {sidebar} {primary} secondary={is_thread ? secondary : undefined} />
