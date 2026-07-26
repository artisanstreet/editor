<script lang="ts">
	import "$lib/styles/fonts.css";
	import "$lib/styles/global.css";
	import "$lib/styles/artisan-compatibility.css";

	import { page } from "$app/state";
	import { ModeWatcher } from "mode-watcher";
	import { onMount } from "svelte";
	import ArtisanSidebar from "./components/artisan-sidebar.sv";
	import SectionedPanel from "./components/sectioned-panel.sv";
	import ThreadPanel from "./components/thread-panel.sv";

	let { children } = $props();
	let desktop_runtime = $state(false);
	const is_thread = $derived(/^\/threads\/[^/]+\/?$/.test(page.url.pathname));

	onMount(() => {
		desktop_runtime = navigator.userAgent.includes("Electron/");
	});
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

<div class="flex h-dvh min-h-0 flex-col bg-background">
	{#if desktop_runtime}
		<div
			aria-hidden="true"
			class="h-10 shrink-0 bg-background"
			style="-webkit-app-region: drag;"
		></div>
	{/if}
	<div class="min-h-0 flex-1">
		<SectionedPanel {sidebar} {primary} secondary={is_thread ? secondary : undefined} />
	</div>
</div>
