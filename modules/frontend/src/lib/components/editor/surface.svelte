<script lang="ts" effect>
	import { Effect } from "effect";
	import { EditorService } from "$lib/editor/service";

	let {
		label = "\u0043ode editor",
	}: { readonly label?: string } = $props();
	let host = $state<HTMLDivElement>();

	const editor = yield* EditorService;
	const Mount = (next_host: HTMLElement) =>
		Effect.gen(function* () {
			yield* editor.Attach(next_host);
			yield* Effect.never;
		}).pipe(Effect.ensuring(editor.Detach));

	/** SER interrupts this fiber when the bound host changes or this component unmounts. */
	if (host !== undefined) yield* Mount(host);
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
