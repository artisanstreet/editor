<script lang="ts">
	import { onMount } from "svelte";

	/**
	 * Rendering stays deliberately thin. The application composition boundary
	 * supplies this bridge after it acquires MonacoEditorService in its Effect
	 * scope, keeping the component free of runtime, filesystem, and Electron APIs.
	 */
	export interface MonacoEditorMount {
		readonly attach: (host: HTMLElement) => () => void;
	}

	let { mount, label = "Code editor" }: { readonly mount: MonacoEditorMount; readonly label?: string } = $props();
	let host = $state<HTMLDivElement>();

	onMount(() => {
		if (host === undefined) return;
		return mount.attach(host);
	});
</script>

<div bind:this={host} class="min-h-0 flex-1 bg-background" role="region" aria-label={label}></div>
