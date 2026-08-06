<script lang="ts">
	import { page } from "$app/state";
	import SettingsEngine from "../../../components/settings/engine.svelte";

	const engine_id = $derived(page.params.engine ?? "");
</script>

<SettingsEngine {engine_id} />
