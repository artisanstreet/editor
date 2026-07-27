<script lang="ts" effect>
	import type { ConversationItem } from "@artisan/protocol";
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import { Effect } from "effect";
	import type { Snippet } from "svelte";
	import { thinking_word_at } from "$lib/conversation/activity-status";

	let {
		activity_label,
		item,
		details,
		duration_kind,
	}: {
		activity_label?: string;
		item: Extract<ConversationItem, { type: "work_session" }>;
		details?: Snippet;
		duration_kind?: "thought" | "worked";
	} = $props();
	let open = $state(false);
	let details_element = $state<HTMLDivElement>();
	let has_visible_details = $state(false);
	let thinking_word_index = $state(0);

	const FormatDuration = (started_at: string, ended_at: string) => {
		const total_seconds = Math.max(
			0,
			Math.floor((Date.parse(ended_at) - Date.parse(started_at)) / 1_000),
		);
		const hours = Math.floor(total_seconds / 3_600);
		const minutes = Math.floor((total_seconds % 3_600) / 60);
		const seconds = total_seconds % 60;

		return [
			hours > 0 ? `${hours}h` : undefined,
			minutes > 0 || hours > 0 ? `${minutes}m` : undefined,
			`${seconds}s`,
		]
			.filter((part) => part !== undefined)
			.join(" ");
	};

	const label = $derived(
		item.ended_at === undefined
			? (activity_label ?? thinking_word_at(thinking_word_index))
			: `${duration_kind === "worked" ? "Worked" : "Thought"} for ${FormatDuration(item.started_at, item.ended_at)}`,
	);
	const is_working = $derived(item.ended_at === undefined);
	const can_collapse = $derived(!is_working && has_visible_details);

	yield* Effect.gen(function* () {
		while (is_working) {
			yield* Effect.sleep("2 seconds");
			if (!is_working) return;
			if (activity_label === undefined) thinking_word_index += 1;
		}
	});

	/** Snippets are opaque; observe their rendered trace rather than treating their presence as content. */
	$effect(() => {
		if (details_element === undefined) {
			has_visible_details = false;
			return;
		}

		const UpdateVisibleDetails = () => {
			has_visible_details = details_element?.childElementCount > 0;
		};
		const observer = new MutationObserver(UpdateVisibleDetails);
		observer.observe(details_element, { childList: true, subtree: true });
		UpdateVisibleDetails();

		return () => observer.disconnect();
	});
</script>

<section
	class="t-acc w-full text-base text-muted-foreground"
	data-open={is_working || can_collapse ? is_working || open : undefined}
	data-state={is_working || can_collapse ? (is_working || open ? "open" : "closed") : undefined}
	data-has-header={can_collapse ? "true" : undefined}
	aria-label={`${label}: ${item.title}`}
>
	{#if can_collapse}
		<button
			class="t-acc-head flex w-full cursor-pointer items-center gap-1 border-b border-border pb-2 text-left"
			type="button"
			aria-expanded={open}
			onclick={() => (open = !open)}
		>
			<span>{label}</span>
			<ChevronRight
				class={`size-4 transition-transform duration-250 ease-[cubic-bezier(0.22,1,0.36,1)] motion-reduce:transition-none ${open ? "rotate-90" : ""}`}
				aria-hidden="true"
			/>
		</button>
	{:else}
		{#if is_working}
			<div
				class="flex w-fit items-center gap-2 py-0.5"
				role="status"
				aria-label={activity_label ?? "Artisan is working"}
			>
				<span class="artisan-working-sprite size-5 shrink-0" aria-hidden="true"></span>
				<span class="motion-reduce:hidden" aria-hidden="true">{label}</span>
				<span class="hidden motion-reduce:inline" aria-hidden="true">
					{activity_label ?? "Working..."}
				</span>
			</div>
		{:else}
			<div class="flex w-full items-center gap-1 border-b border-border pb-2">
				<span>{label}</span>
			</div>
		{/if}
	{/if}

	{#if details !== undefined}
		<div class="t-acc-panel" hidden={!is_working && !has_visible_details}>
			<div class="t-acc-panel-inner" bind:this={details_element}>
				{@render details()}
			</div>
		</div>
	{/if}
</section>

<style>
	.artisan-working-sprite {
		background-image: url("/activity/artisan-working-sprite.png");
		background-repeat: no-repeat;
		background-position: 0 0;
		background-size: 200% 200%;
		image-rendering: pixelated;
		animation: artisan-working-frames 1600ms steps(1, end) infinite;
	}

	@keyframes artisan-working-frames {
		0%,
		100% {
			background-position: 0 0;
		}

		25% {
			background-position: 100% 0;
		}

		50% {
			background-position: 0 100%;
		}

		75% {
			background-position: 100% 100%;
		}
	}

	.t-acc-panel {
		display: grid;
		grid-template-rows: 0fr;
		transition: grid-template-rows 250ms cubic-bezier(0.22, 1, 0.36, 1);
	}

	.t-acc[data-open="true"] .t-acc-panel {
		grid-template-rows: 1fr;
	}

	.t-acc-panel-inner {
		overflow: hidden;
		opacity: 0;
		filter: blur(2px);
		transition:
			opacity 250ms cubic-bezier(0.22, 1, 0.36, 1),
			filter 250ms cubic-bezier(0.22, 1, 0.36, 1);
	}

	.t-acc[data-has-header="true"][data-open="true"] .t-acc-panel-inner {
		padding-top: 1rem;
	}

	.t-acc[data-open="true"] .t-acc-panel-inner {
		opacity: 1;
		filter: blur(0);
	}

	@media (prefers-reduced-motion: reduce) {
		.artisan-working-sprite {
			animation: none;
			background-position: 0 0;
		}

		.t-acc-panel,
		.t-acc-panel-inner {
			transition: none !important;
		}
	}
</style>
