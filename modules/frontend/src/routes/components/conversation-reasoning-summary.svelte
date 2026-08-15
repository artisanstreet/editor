<script lang="ts" effect>
	import { Comark } from "@comark/svelte";
	import { thinking_word_for } from "$lib/conversation/activity-status";
	import { ShimmerText } from "$lib/components/ui/shimmer-text";
	import { conversation_parse_options } from "$lib/components/markdown/parsing";
	import type { ConversationReasoningItem } from "$lib/conversation/trace";

	let {
		items,
		live,
	}: {
		items: ReadonlyArray<ConversationReasoningItem>;
		live: boolean;
	} = $props();

	const thinking_word = $derived(thinking_word_for(items[0]?.run_id ?? "reasoning"));
	const single_item = $derived(items.length === 1 ? items[0] : undefined);
	const trace_items = $derived(items.slice(1));

	/** Replay the entrance when this compact, live group mounts without a timer. */
	const reveal = (node: HTMLElement) => {
		node.classList.remove("is-hiding");
		node.classList.remove("is-shown");
		void node.offsetHeight;
		node.classList.add("is-shown");

		return {
			destroy: () => node.classList.remove("is-shown", "is-hiding"),
		};
	};
</script>

{#if single_item !== undefined}
	<div class="t-stagger max-w-(--prose-body-width)" use:reveal>
		<div class="t-stagger-line">
			<Comark
				class="prose conversation-reasoning text-base leading-7 text-muted-foreground"
				markdown={single_item.text}
				options={conversation_parse_options}
			/>
		</div>
	</div>
{:else if items.length > 1}
	<article
		class="relative max-w-(--prose-body-width) pl-6"
		aria-label={`${thinking_word} reasoning trace`}
	>
		<div
			class="pointer-events-none absolute inset-y-0 left-0 w-4 after:absolute after:inset-y-0 after:left-1/2 after:w-px after:-translate-x-1/2 after:bg-border/60"
			aria-hidden="true"
		></div>
		<div class="flex flex-col gap-1">
			<div class="t-stagger" use:reveal>
				<div class="t-stagger-line">
					<ShimmerText active={live} class="text-base text-muted-foreground">
						{thinking_word}
					</ShimmerText>
				</div>
			</div>
			{#each trace_items as summary, index (summary.id)}
				<div class="t-stagger" use:reveal>
					<div
						class="t-stagger-line"
						class:t-stagger-line--2={index === 0}
						style={`transition-delay: calc(var(--stagger-stagger) * ${index + 1});`}
					>
						<Comark
							class="prose conversation-reasoning text-base leading-7 text-muted-foreground"
							markdown={summary.text}
							options={conversation_parse_options}
						/>
					</div>
				</div>
			{/each}
		</div>
	</article>
{/if}

<style>
	:global(:root) {
		--stagger-dur: 500ms;
		--stagger-distance: 12px;
		--stagger-stagger: 40ms;
		--stagger-blur: 3px;
		--stagger-ease: cubic-bezier(0.22, 1, 0.36, 1);
	}

	/* Lines start translated down + blurred + invisible; .is-shown
	   on the parent flips them to their resting state. The second
	   line's transition-delay holds it back by --stagger-stagger
	   so the eye lands on the headline first. */
	.t-stagger-line {
		display: block;
		opacity: 0;
		transform: translateY(var(--stagger-distance));
		filter: blur(var(--stagger-blur));
		transition:
			opacity   var(--stagger-dur) var(--stagger-ease),
			transform var(--stagger-dur) var(--stagger-ease),
			filter    var(--stagger-dur) var(--stagger-ease);
		will-change: transform, opacity, filter;
	}
	.t-stagger-line--2 { transition-delay: var(--stagger-stagger); }

	.t-stagger:global(.is-shown) .t-stagger-line {
		opacity: 1;
		transform: translateY(0);
		filter: blur(0);
	}
	/* Exit decouples from the stagger: same fade for every line,
	   no Y return, no blur — so the disappearance reads as a
	   single quiet fade instead of a reverse reveal. */
	.t-stagger:global(.is-hiding) .t-stagger-line {
		opacity: 0;
		transform: translateY(0);
		filter: blur(0);
		transition:
			opacity 200ms ease,
			transform 0s linear,
			filter 0s linear;
		transition-delay: 0s;
	}

	@media (prefers-reduced-motion: reduce) {
		.t-stagger-line { transition: none !important; }
	}
</style>
