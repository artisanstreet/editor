<script lang="ts" effect>
	import type {
		RuntimeCatalog,
		SurfaceUsageAggregate,
		ThreadSessionPolicy,
	} from "@artisan/protocol";
	import ArrowUp from "@tabler/icons-svelte/icons/arrow-up";
	import MessagePlus from "@tabler/icons-svelte/icons/message-plus";
	import PlayerStopFilled from "@tabler/icons-svelte/icons/player-stop-filled";
	import { Effect } from "effect";
	import { Button } from "$lib/components/ui/button";
	import {
		Tooltip,
		TooltipContent,
		TooltipProvider,
		TooltipTrigger,
	} from "$lib/components/ui/tooltip";
	import { ContextUsageAutoCompactionPercent } from "$lib/context-usage/auto-compaction";
	import { ContextUsageModelName } from "$lib/context-usage/model-name";
	import ContextUsageGauge from "../context-usage-gauge.svelte";
	import ModelSelector from "../model-selector/view.svelte";

	let {
		abort_available,
		cancelling,
		context_percent,
		context_usage,
		context_window_tokens,
		disabled,
		engine_locked,
		new_thread_ready = false,
		onpolicychange,
		onprimaryaction,
		onstartnewthread,
		policy,
		run_active,
		runtime_catalog,
		send_blocked_reason,
		send_ready,
	}: {
		abort_available: boolean;
		cancelling: boolean;
		context_percent?: number;
		context_usage?: SurfaceUsageAggregate;
		context_window_tokens?: number;
		disabled: boolean;
		engine_locked: boolean;
		new_thread_ready?: boolean;
		onpolicychange?: (
			policy: ThreadSessionPolicy,
		) => Effect.Effect<ThreadSessionPolicy, { readonly message: string }>;
		onprimaryaction: Effect.Effect<void>;
		onstartnewthread?: Effect.Effect<void>;
		policy?: ThreadSessionPolicy;
		run_active: boolean;
		runtime_catalog: RuntimeCatalog;
		send_blocked_reason?: string;
		send_ready: boolean;
	} = $props();

	/** The action only exists while a run holds the primary button hostage. */
	const new_thread_open = $derived(new_thread_ready && onstartnewthread !== undefined);
	const ActivateNewThread = Effect.gen(function* () {
		if (onstartnewthread !== undefined) yield* onstartnewthread;
	});

	/** All three are needed to state a reading; any one missing means no gauge at all. */
	const has_context_reading = $derived(
		context_usage?.context_tokens !== undefined &&
			context_window_tokens !== undefined &&
			context_percent !== undefined,
	);
	/**
	 * Where the reporting run's engine begins compacting, which the gauge's red
	 * leg runs up to. The selector policy only describes a subsequent launch and
	 * must not reinterpret already-reported telemetry.
	 */
	const compaction_percent = $derived(
		ContextUsageAutoCompactionPercent(context_usage, context_window_tokens ?? 0),
	);
	/** The gauge names the immutable reporting run, never the next-launch policy. */
	const context_model_name = $derived(
		context_usage === undefined ? undefined : ContextUsageModelName(context_usage, runtime_catalog),
	);
</script>

<div class="flex items-center justify-between gap-2">
	<!--
		The gauge stands beside the model whose window it measures, not inside the
		picker and not beside the send button, where it sat next to an action it
		says nothing about. Its own control again, so it carries its own tooltip.
	-->
	<div class="flex min-w-0 items-center gap-0.5">
		<ModelSelector
			{disabled}
			{engine_locked}
			{onpolicychange}
			{policy}
			{runtime_catalog}
		/>
		{#if has_context_reading && context_percent !== undefined && context_usage !== undefined && context_window_tokens !== undefined}
			<ContextUsageGauge
				{compaction_percent}
				model_name={context_model_name}
				percent={context_percent}
				usage={context_usage}
				window_tokens={context_window_tokens}
			/>
		{/if}
	</div>
	<div class="flex items-center">
		<!--
			Extends leftward from the stop button while a run occupies it: the typed
			draft is not stuck behind the run — it can open its own thread. The grid
			track tween keeps the reveal purely CSS and fully reversible mid-flight.
		-->
		<div class="t-new-thread" data-open={new_thread_open}>
			<div class="t-new-thread-inner">
				<Button
					variant="ghost"
					size="sm"
					class="new-thread-action mr-1 gap-1.5 whitespace-nowrap rounded-[calc(var(--composer-radius)-0.5rem)] text-muted-foreground hover:text-foreground"
					disabled={!new_thread_open}
					tabindex={new_thread_open ? 0 : -1}
					onclick={yield* ActivateNewThread}
				>
					<MessagePlus class="size-4" aria-hidden="true" />
					<span>Start a new thread with prompt</span>
				</Button>
			</div>
		</div>
		<TooltipProvider delayDuration={0}>
			<Tooltip>
				<TooltipTrigger>
					{#snippet child({ props: tooltip_props })}
						<span {...tooltip_props} class="flex has-[:disabled]:cursor-not-allowed">
							<!--
								Sized to match the model picker rather than to the icon
								button default: the two sit on one row at opposite ends,
								so a height mismatch between them reads as the row being
								crooked. Same box, same inset, one bar.
							-->
							<Button
								variant="ghost"
								size="icon-sm"
								class="composer-send rounded-[calc(var(--composer-radius)-0.5rem)]"
								aria-label={run_active ? "Stop current run" : "Send message"}
								data-ready={run_active || send_ready}
								disabled={run_active ? disabled || cancelling || !abort_available : !send_ready}
								onclick={yield* onprimaryaction}
							>
								<span
									class="t-icon-swap size-4"
									data-state={run_active ? "b" : "a"}
									aria-hidden="true"
								>
									<span class="t-icon" data-icon="a"><ArrowUp class="size-4" /></span>
									<span class="t-icon" data-icon="b"
										><PlayerStopFilled class="size-4" /></span
									>
								</span>
							</Button>
						</span>
					{/snippet}
				</TooltipTrigger>
				{#if send_blocked_reason !== undefined && !run_active}
					<TooltipContent>{send_blocked_reason}</TooltipContent>
				{/if}
			</Tooltip>
		</TooltipProvider>
	</div>
</div>

<style>
	:global(:root) {
		--icon-swap-dur: var(--duration-fast);
		--icon-swap-blur: 2px;
		--icon-swap-start-scale: 0.25;
		--icon-swap-ease: var(--ease-in-out);
	}

	:global(.t-icon-swap) {
		position: relative;
		display: inline-grid;
	}

	:global(.t-icon-swap) .t-icon {
		grid-area: 1 / 1;
		transition:
			opacity var(--icon-swap-dur) var(--icon-swap-ease),
			filter var(--icon-swap-dur) var(--icon-swap-ease),
			transform var(--icon-swap-dur) var(--icon-swap-ease);
		will-change: opacity, filter, transform;
	}

	:global(.t-icon-swap[data-state="a"]) .t-icon[data-icon="a"],
	:global(.t-icon-swap[data-state="b"]) .t-icon[data-icon="b"] {
		opacity: 1;
		filter: blur(0);
		transform: scale(1);
	}

	:global(.t-icon-swap[data-state="a"]) .t-icon[data-icon="b"],
	:global(.t-icon-swap[data-state="b"]) .t-icon[data-icon="a"] {
		opacity: 0;
		filter: blur(var(--icon-swap-blur));
		transform: scale(var(--icon-swap-start-scale));
	}

	/**
	 * Card-resize reveal for the run-time "start a new thread" action: the grid
	 * track tweens 0fr → 1fr so the row grows leftward from the stop button,
	 * while the label blurs in slightly after the track starts opening.
	 */
	.t-new-thread {
		display: grid;
		grid-template-columns: 0fr;
		transition: grid-template-columns var(--duration-fast) var(--ease-smooth-out);
	}

	.t-new-thread[data-open="true"] {
		grid-template-columns: 1fr;
	}

	.t-new-thread-inner {
		display: flex;
		min-width: 0;
		overflow: hidden;
	}

	.t-new-thread :global(.new-thread-action) {
		opacity: 0;
		filter: blur(var(--blur-small));
		transform: translateX(var(--distance-micro));
		transition:
			opacity var(--duration-quick) var(--ease-smooth-out),
			filter var(--duration-quick) var(--ease-smooth-out),
			transform var(--duration-quick) var(--ease-smooth-out);
		will-change: opacity, filter, transform;
	}

	.t-new-thread[data-open="true"] :global(.new-thread-action) {
		opacity: 1;
		filter: blur(0);
		transform: translateX(0);
		transition-duration: var(--duration-fast);
		transition-delay: var(--duration-micro);
	}

	/** The face arms from bare arrow to bright gradient when submission becomes possible. */
	:global(.composer-send) {
		position: relative;
		background: transparent;
		transition: filter var(--duration-quick) var(--ease-in-out);
	}

	:global(.composer-send)::before {
		content: "";
		position: absolute;
		inset: 0;
		border-radius: inherit;
		background: linear-gradient(to bottom, var(--surface-25), var(--surface-100));
		clip-path: circle(0% at 50% 50%);
		transition: clip-path var(--duration-quick) var(--ease-smooth-out);
		will-change: clip-path;
	}

	:global(.composer-send[data-ready="true"])::before {
		clip-path: circle(75% at 50% 50%);
		transition-duration: var(--duration-fast);
	}

	:global(.composer-send)::after {
		content: "";
		position: absolute;
		inset: 0;
		border-radius: inherit;
		pointer-events: none;
		box-shadow: none;
		transition: box-shadow var(--duration-quick) var(--ease-smooth-out);
	}

	:global(.composer-send[data-ready="true"])::after {
		box-shadow: var(--shadow-inset-artwork);
		transition-duration: var(--duration-fast);
	}

	:global(.composer-send:hover:not(:disabled)) { filter: brightness(0.96); }
	:global(.composer-send:active:not(:disabled)) { filter: brightness(0.9); }
	:global(.composer-send[data-ready="true"]:disabled) { filter: brightness(0.8); }

	:global(.composer-send .t-icon-swap) {
		color: #a1a1aa;
		transition: color var(--duration-quick) var(--ease-in-out);
	}

	:global(.composer-send[data-ready="true"] .t-icon-swap) { color: #18181b; }

	@media (prefers-reduced-motion: reduce) {
		:global(.t-icon-swap) .t-icon,
		:global(.composer-send),
		:global(.composer-send)::before,
		:global(.composer-send)::after,
		.t-new-thread,
		.t-new-thread :global(.new-thread-action) {
			transition: none !important;
			will-change: auto;
		}
	}
</style>
