<script lang="ts">
	import { GetConversationActivityPresentation, type ConversationItem } from "@artisan/protocol";
	import AlertTriangle from "@tabler/icons-svelte/icons/alert-triangle";
	import Bug from "@tabler/icons-svelte/icons/bug";
	import Terminal2 from "@tabler/icons-svelte/icons/terminal-2";
	import Tool from "@tabler/icons-svelte/icons/tool";
	import WorldSearch from "@tabler/icons-svelte/icons/world-search";
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import { conversation_activity_is_live } from "$lib/conversation/activity-status";
	import { conversation_diagnostics_enabled } from "$lib/conversation/diagnostics";
	import { ShimmerText } from "$lib/components/ui/shimmer-text";
	import {
		make_conversation_trace_segments,
		type ConversationActivityItem,
	} from "$lib/conversation/trace";
	import ConversationItemView from "./conversation-item.sv";

	let {
		failed = false,
		items,
		work_active = false,
	}: {
		/** Failed work must explain itself: diagnostics render open and unmuted. */
		failed?: boolean;
		items: ReadonlyArray<ConversationItem>;
		/** A stale provider item cannot keep animating after its owning work has settled. */
		work_active?: boolean;
	} = $props();
	let open_groups = $state<Record<string, boolean>>({});

	const segments = $derived(
		make_conversation_trace_segments(items, $conversation_diagnostics_enabled, failed),
	);

	const IsCommandGroup = (activities: ReadonlyArray<ConversationActivityItem>) =>
		activities.every((activity) => activity.kind === "terminal_activity");

	const GroupLabel = (activities: ReadonlyArray<ConversationActivityItem>) => {
		if (IsCommandGroup(activities)) return activities.length === 1 ? "Ran a command" : "Ran commands";
		if (activities.length === 1) return "Used a tool";
		return "Used tools";
	};

	/** The newest still-running activity, whose live label fronts its group. */
	const LiveActivity = (activities: ReadonlyArray<ConversationActivityItem>) =>
		work_active ? activities.findLast(conversation_activity_is_live) : undefined;

	const ToggleGroup = (id: string) => {
		open_groups[id] = !open_groups[id];
	};
</script>

{#if segments.length > 0}
	<div class="flex flex-col gap-5">
		{#each segments as segment (segment.id)}
			{#if segment.type === "item"}
				<ConversationItemView item={segment.item} />
			{:else if segment.type === "activity_group"}
				{@const open = open_groups[segment.id] ?? false}
				{@const live = LiveActivity(segment.items)}
				<div
					class="trace-acc flex flex-col"
					data-open={open}
					data-state={open ? "open" : "closed"}
				>
					<button
						type="button"
						class="trace-acc-head flex w-fit cursor-pointer items-center gap-2 py-0.5 text-base text-muted-foreground transition-colors duration-150 hover:text-foreground motion-reduce:transition-none"
						aria-expanded={open}
						onclick={() => ToggleGroup(segment.id)}
					>
						<Terminal2 class="size-4" aria-hidden="true" />
						{#if live !== undefined}
							<ShimmerText class="text-muted-foreground">
							{GetConversationActivityPresentation(live).label}
						</ShimmerText>
						{:else}
							<span>{GroupLabel(segment.items)}</span>
						{/if}
						<span class="trace-acc-chevron flex">
							<ChevronRight class="size-3.5" aria-hidden="true" />
						</span>
					</button>

					<div class="trace-acc-panel">
						<div class="trace-acc-panel-inner flex flex-col gap-1 pt-1">
							{#each segment.items as activity (activity.id)}
								<div class="flex min-w-0 items-center gap-2 py-0.5 text-base text-muted-foreground">
									{#if activity.kind === "terminal_activity"}
										<Terminal2 class="size-4 shrink-0" aria-hidden="true" />
									{:else if activity.kind === "search"}
										<WorldSearch class="size-4 shrink-0" aria-hidden="true" />
									{:else}
										<Tool class="size-4 shrink-0" aria-hidden="true" />
									{/if}
									<span class="min-w-0 truncate">
										{activity.kind === "terminal_activity" && activity.detail !== undefined
											? `Ran ${activity.detail}`
											: (activity.detail ?? activity.label)}
									</span>
								</div>
							{/each}
						</div>
					</div>
				</div>
			{:else}
				{@const open = open_groups[segment.id] ?? failed}
				<div
					class="trace-acc flex flex-col"
					data-open={open}
					data-state={open ? "open" : "closed"}
					role={failed ? "alert" : undefined}
				>
					<button
						type="button"
						class={`trace-acc-head flex w-fit cursor-pointer items-center gap-2 py-0.5 text-base transition-colors duration-150 motion-reduce:transition-none ${failed ? "text-destructive hover:text-destructive" : "text-muted-foreground hover:text-foreground"}`}
						aria-expanded={open}
						onclick={() => ToggleGroup(segment.id)}
					>
						{#if failed}
							<AlertTriangle class="size-4" aria-hidden="true" />
							<span>Failure details</span>
						{:else}
							<Bug class="size-4" aria-hidden="true" />
							<span>Diagnostics</span>
						{/if}
						<span class="trace-acc-chevron flex">
							<ChevronRight class="size-3.5" aria-hidden="true" />
						</span>
					</button>

					<div class="trace-acc-panel">
						<div class="trace-acc-panel-inner flex flex-col gap-1 pt-1">
							{#each segment.items as diagnostic (diagnostic.id)}
								<div
									class={`flex min-w-0 items-start gap-2 py-0.5 text-sm ${failed ? "text-destructive" : "text-muted-foreground"}`}
								>
									{#if failed}
										<AlertTriangle class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
									{:else}
										<Bug class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
									{/if}
									<span class="min-w-0 break-words">{diagnostic.summary}</span>
								</div>
							{/each}
						</div>
					</div>
				</div>
			{/if}
		{/each}
	</div>
{/if}

<style>
	.trace-acc-panel {
		display: grid;
		grid-template-rows: 0fr;
		transition: grid-template-rows 250ms cubic-bezier(0.22, 1, 0.36, 1);
	}

	.trace-acc[data-open="true"] .trace-acc-panel {
		grid-template-rows: 1fr;
	}

	.trace-acc-panel-inner {
		overflow: hidden;
		opacity: 0;
		filter: blur(2px);
		transition:
			opacity 250ms cubic-bezier(0.22, 1, 0.36, 1),
			filter 250ms cubic-bezier(0.22, 1, 0.36, 1);
	}

	.trace-acc[data-open="true"] .trace-acc-panel-inner {
		opacity: 1;
		filter: blur(0);
	}

	.trace-acc-chevron {
		transform: rotate(0deg);
		transform-origin: center;
		transition: transform 250ms cubic-bezier(0.22, 1, 0.36, 1);
	}

	.trace-acc[data-open="true"] .trace-acc-chevron {
		transform: rotate(90deg);
	}

	@media (prefers-reduced-motion: reduce) {
		.trace-acc-panel,
		.trace-acc-panel-inner,
		.trace-acc-chevron {
			transition: none !important;
		}
	}
</style>
