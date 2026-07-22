<script lang="ts">
	import "$lib/styles/fonts.css";
	import "$lib/styles/global.css";
	import "$lib/styles/artisan-compatibility.css";

	import { ModeWatcher } from "mode-watcher";
	import AppShell from "./components/app-shell.sv";

	let { children } = $props();
</script>

<ModeWatcher defaultMode="dark" />
<AppShell>{@render children()}</AppShell>
