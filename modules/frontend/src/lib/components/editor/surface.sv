<script lang="ts">
	import { onMount } from "svelte";

	/**
	 * Rendering stays deliberately thin. The application composition boundary
	 * supplies this bridge after it acquires EditorService in its Effect scope,
	 * keeping the component free of runtime, filesystem, and Electron APIs.
	 */
	export interface EditorSurfaceMount {
		readonly attach: (host: HTMLElement) => () => void;
	}

	let {
		mount,
		label = "Code editor",
	}: { readonly mount: EditorSurfaceMount; readonly label?: string } = $props();
	let host = $state<HTMLDivElement>();

	onMount(() => {
		if (host === undefined) return;
		return mount.attach(host);
	});
</script>

<div
	bind:this={host}
	class="editor-surface min-h-0 flex-1 overflow-hidden"
	role="region"
	aria-label={label}
></div>

<style>
	.editor-surface :global(.cm-editor) {
		height: 100%;
	}

	/*
	 * The document fades into the card's edges the way the transcript and the
	 * sidebar do. The mask has to sit on CodeMirror's own scroller, since the
	 * fade is driven by that element's scroll position.
	 */
	.editor-surface :global(.cm-scroller) {
		scrollbar-width: thin;
		scrollbar-color: var(--surface-500) transparent;
		--docs-scroll-fade-size: 24px;
		-webkit-mask-image: linear-gradient(
			to bottom,
			transparent,
			black var(--docs-scroll-fade-start),
			black calc(100% - var(--docs-scroll-fade-end)),
			transparent
		);
		mask-image: linear-gradient(
			to bottom,
			transparent,
			black var(--docs-scroll-fade-start),
			black calc(100% - var(--docs-scroll-fade-end)),
			transparent
		);
	}

	@supports (animation-timeline: scroll()) {
		.editor-surface :global(.cm-scroller) {
			animation:
				docs-scroll-fade-start linear both,
				docs-scroll-fade-end linear both;
			animation-timeline: scroll(self block), scroll(self block);
			animation-range:
				0 var(--docs-scroll-fade-size),
				calc(100% - var(--docs-scroll-fade-size)) 100%;
		}
	}
</style>
