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
	import { ComposerContextUsageIsCurrent } from "$lib/composer/send-readiness";
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

	/**
	 * All three are needed to state a reading; any one missing means no gauge at
	 * all. So is a reading that belongs to a different engine or model than the
	 * thread now targets: a stale numerator over a fresh denominator is not a
	 * conservative reading, it is a wrong one, and it was what put a 252K window
	 * beside a picker reading 1M. No gauge until the current pairing reports.
	 */
	const has_context_reading = $derived(
		context_usage?.context_tokens !== undefined &&
			context_window_tokens !== undefined &&
			context_percent !== undefined &&
			ComposerContextUsageIsCurrent(policy, context_usage),
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
		<!--
			Card-resize reveal for the run-time "start a new thread" action: the grid
			track tweens 0fr → 1fr so the row grows leftward from the stop button,
			while the label blurs in behind it.
		-->
		<div
			class="group/new-thread grid grid-cols-[0fr] transition-[grid-template-columns] duration-(--duration-fast) ease-(--ease-smooth-out) data-[open=true]:grid-cols-[1fr]"
			data-open={new_thread_open}
		>
			<div class="flex min-w-0 overflow-hidden">
				<Button
					variant="ghost"
					size="sm"
					class="mr-1 gap-1.5 rounded-(--radius-nested) whitespace-nowrap text-muted-foreground opacity-0 blur-[var(--blur-small)] [transition-property:opacity,filter] duration-(--duration-quick) ease-(--ease-smooth-out) group-data-[open=true]/new-thread:opacity-100 group-data-[open=true]/new-thread:blur-none group-data-[open=true]/new-thread:duration-(--duration-fast) hover:text-foreground"
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
								class="composer-send rounded-(--radius-nested)"
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
