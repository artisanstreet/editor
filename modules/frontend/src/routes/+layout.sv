<script lang="ts">
	import "$lib/styles/global.css";

	import { ModeWatcher } from "mode-watcher";
	import AppShell from "./components/app-shell.sv";

	let { children } = $props();
</script>

<ModeWatcher defaultMode="dark" />
<AppShell>{@render children()}</AppShell>
