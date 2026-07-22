<script lang="ts">
	import "$lib/styles/fonts.css";
	import "$lib/styles/global.css";
	import "$lib/styles/artisan-compatibility.css";

	import { ModeWatcher } from "mode-watcher";
	import ArtisanSidebar from "./components/artisan-sidebar.sv";
	import SectionedPanel from "./components/sectioned-panel.sv";

	let { children } = $props();
</script>

<ModeWatcher defaultMode="dark" />
<SectionedPanel>
	{#snippet sidebar()}
		<ArtisanSidebar />
	{/snippet}

	{#snippet primary()}
		{@render children()}
	{/snippet}
</SectionedPanel>
