<script lang="ts" effect>
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import type { Effect } from "effect";
	import { thinking_word_for } from "$lib/conversation/activity-status";
	import { ShimmerText } from "$lib/components/ui/shimmer-text";
	import type { ConversationReasoningItem } from "$lib/conversation/trace";

	let {
		items,
		live,
		ontoggle,
		open,
	}: {
		items: ReadonlyArray<ConversationReasoningItem>;
		live: boolean;
		ontoggle: () => Effect.Effect<void>;
		open: boolean;
	} = $props();

	const thinking_word = $derived(thinking_word_for(items[0]?.run_id ?? "reasoning"));
</script>

<article
	class="reasoning-acc flex max-w-(--prose-body-width) flex-col"
	data-open={open}
	data-state={open ? "open" : "closed"}
	aria-label={`${thinking_word} reasoning summary`}
>
	<button
		type="button"
		class="t-acc-head flex w-fit max-w-full cursor-pointer items-center gap-1 py-0.5 text-left text-base text-muted-foreground transition-colors duration-150 hover:text-foreground disabled:pointer-events-none motion-reduce:transition-none"
		aria-expanded={open}
		onclick={yield* ontoggle()}
	>
		<!-- Keep this node stable: lifecycle changes stop the shimmer without remounting history. -->
		<ShimmerText active={live} class="min-w-0 truncate text-inherit">
			{thinking_word}
		</ShimmerText>
		<span
			class="reasoning-acc-chevron flex shrink-0"
		>
			<ChevronRight class="size-3.5" aria-hidden="true" />
		</span>
	</button>

	<div class="reasoning-acc-panel">
		<div class="reasoning-acc-panel-inner pt-1">
			<div class="ml-2 flex flex-col gap-2 border-l border-border/60 py-0.5 pl-4">
				{#each items as summary (summary.id)}
					<p class="whitespace-pre-wrap text-base leading-7 text-muted-foreground">
						{summary.text}
					</p>
				{/each}
			</div>
		</div>
	</div>
</article>

<style>
	.reasoning-acc-panel {
		display: grid;
		grid-template-rows: 0fr;
		transition: grid-template-rows 250ms cubic-bezier(0.22, 1, 0.36, 1);
	}

	.reasoning-acc[data-open="true"] .reasoning-acc-panel {
		grid-template-rows: 1fr;
	}

	.reasoning-acc-panel-inner {
		overflow: hidden;
		opacity: 0;
		filter: blur(2px);
		transition:
			opacity 250ms cubic-bezier(0.22, 1, 0.36, 1),
			filter 250ms cubic-bezier(0.22, 1, 0.36, 1);
	}

	.reasoning-acc[data-open="true"] .reasoning-acc-panel-inner {
		opacity: 1;
		filter: blur(0);
	}

	.reasoning-acc-chevron {
		transform: rotate(0deg);
		transform-origin: center;
		transition: transform 250ms cubic-bezier(0.22, 1, 0.36, 1);
	}

	.reasoning-acc[data-open="true"] .reasoning-acc-chevron {
		transform: rotate(90deg);
	}

	@media (prefers-reduced-motion: reduce) {
		.reasoning-acc-panel,
		.reasoning-acc-panel-inner,
		.reasoning-acc-chevron {
			transition: none !important;
		}
	}
</style>
