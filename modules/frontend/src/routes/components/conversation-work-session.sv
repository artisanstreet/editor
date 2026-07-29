<script lang="ts">
	import type { ConversationItem } from "@artisan/protocol";
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import type { Snippet } from "svelte";
	import { thinking_word_for } from "$lib/conversation/activity-status";
	import { ShimmerText } from "$lib/components/ui/shimmer-text";
	import { EngineMarkClass, EngineMarkFor } from "$lib/engine/presentation";

	let {
		activity_label,
		engine_id,
		item,
		details,
		duration_kind,
	}: {
		activity_label?: string;
		/** Names the engine whose mark spins while this session works. */
		engine_id?: string;
		item: Extract<ConversationItem, { type: "work_session" }>;
		details?: Snippet;
		duration_kind?: "thought" | "worked";
	} = $props();
	/** Failed work opens by default: its explanation must not hide behind a click. */
	let open = $state(item.status === "failed" || item.status === "cancelled");
	let details_element = $state<HTMLDivElement>();
	let has_visible_details = $state(false);

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

	const is_failed = $derived(item.status === "failed");
	const is_cancelled = $derived(item.status === "cancelled");
	const engine_mark = $derived(EngineMarkFor(engine_id));
	/** One word for this session's whole life, chosen from its own identity. */
	const thinking_word = $derived(thinking_word_for(item.id));
	const label = $derived(
		item.ended_at === undefined
			? (activity_label ?? thinking_word)
			: is_failed
				? `Failed after ${FormatDuration(item.started_at, item.ended_at)}`
				: is_cancelled
					? `Cancelled after ${FormatDuration(item.started_at, item.ended_at)}`
					: `${duration_kind === "worked" ? "Worked" : "Thought"} for ${FormatDuration(item.started_at, item.ended_at)}`,
	);
	const is_working = $derived(item.ended_at === undefined);
	const can_collapse = $derived(!is_working && has_visible_details);

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
			<span class={is_failed ? "text-destructive" : ""}>{label}</span>
			<ChevronRight
				class={`size-4 transition-transform duration-250 ease-[cubic-bezier(0.22,1,0.36,1)] motion-reduce:transition-none ${open ? "rotate-90" : ""}`}
				aria-hidden="true"
			/>
		</button>
	{:else}
		{#if is_working}
			{@const EngineIcon = engine_mark.icon}
			<div
				class="flex w-fit items-center gap-2 py-0.5"
				role="status"
				aria-label={activity_label ?? "Artisan is working"}
			>
				<span class="engine-working-mark inline-flex shrink-0" aria-hidden="true">
					<EngineIcon class={EngineMarkClass(engine_mark)} />
				</span>
				<ShimmerText class="text-base" aria-hidden="true">{label}</ShimmerText>
			</div>
		{:else}
			<div class="flex w-full items-center gap-1 border-b border-border pb-2">
				<span class={is_failed ? "text-destructive" : ""}>{label}</span>
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
	/**
	 * The provider mark spins while its engine works, so the running engine is
	 * legible at a glance instead of a generic Artisan sprite.
	 */
	.engine-working-mark {
		animation: engine-working-spin 1400ms linear infinite;
	}

	@keyframes engine-working-spin {
		from {
			transform: rotate(0deg);
		}
		to {
			transform: rotate(360deg);
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
		.engine-working-mark {
			animation: none !important;
		}

		.t-acc-panel,
		.t-acc-panel-inner {
			transition: none !important;
		}
	}
</style>
