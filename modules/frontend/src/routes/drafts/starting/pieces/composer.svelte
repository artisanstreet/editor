<script lang="ts">
	/**
	 * A stand-in for the real composer, faithful only where a landing layout can
	 * be decided by it: the card's footprint, and the control row along its
	 * bottom edge — which is the one strip of the surface that several of these
	 * drafts want to hand to the project picker.
	 *
	 * `leading` is that strip's left end. A variant that puts project selection
	 * anywhere else passes nothing and the row falls back to the model alone.
	 */
	import ArrowUp from "@tabler/icons-svelte/icons/arrow-up";
	import ChevronDown from "@tabler/icons-svelte/icons/chevron-down";
	import type { Snippet } from "svelte";

	let {
		class: klass = "",
		leading,
		placeholder = "Describe a change, or ask about the code",
		tall = false,
	}: {
		class?: string;
		leading?: Snippet;
		placeholder?: string;
		tall?: boolean;
	} = $props();
</script>

<div
	class={`card radius-surface w-full overflow-hidden border border-border bg-linear-to-b from-surface-875 to-surface-900 ${klass}`}
	style:--radius-gap="0.5rem"
	style:--radius-surface="1rem"
>
	<div
		class={`px-4 pt-3.5 text-sm text-muted-foreground ${tall ? "pb-6 min-h-24" : "pb-3 min-h-14"}`}
	>
		{placeholder}
	</div>
	<div class="flex items-center gap-2 px-2.5 pb-2.5">
		{#if leading}
			{@render leading()}
			<span class="h-4 w-px shrink-0 bg-border"></span>
		{/if}
		<button
			class="dz-press dz-focus flex min-w-0 items-center gap-1.5 rounded-(--radius-nested) px-2 py-1.5 text-xs text-foreground outline-none hover:bg-accent"
			type="button"
		>
			<span aria-hidden="true" class="text-[11px]" style:color="oklch(0.72 0.14 45)">✳</span>
			<span class="truncate">Claude Opus 5</span>
			<span class="text-muted-foreground">High</span>
			<ChevronDown class="size-3.5 shrink-0 text-muted-foreground" />
		</button>
		<div class="flex-1"></div>
		<button
			aria-label="Send"
			class="dz-press dz-focus dz-well grid size-7 shrink-0 place-items-center rounded-(--radius-nested) text-foreground outline-none"
			type="button"
		>
			<ArrowUp class="size-4" />
		</button>
	</div>
</div>
