<script lang="ts" effect>
	import type {
		RuntimeCatalog,
		SurfaceUsageAggregate,
		ThreadSessionPolicy,
	} from "@artisan/protocol";
	import ArrowUp from "@tabler/icons-svelte/icons/arrow-up";
	import PlayerStopFilled from "@tabler/icons-svelte/icons/player-stop-filled";
	import type { Effect } from "effect";
	import { Button } from "$lib/components/ui/button";
	import {
		Tooltip,
		TooltipContent,
		TooltipProvider,
		TooltipTrigger,
	} from "$lib/components/ui/tooltip";
	import ContextUsageRing from "../context-usage-ring.sv";
	import ModelSelector from "../model-selector/view.sv";

	let {
		abort_available,
		cancelling,
		context_percent,
		context_usage,
		context_window_tokens,
		disabled,
		engine_locked,
		onpolicychange,
		onprimaryaction,
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
		onpolicychange?: (
			policy: ThreadSessionPolicy,
		) => Effect.Effect<ThreadSessionPolicy, { readonly message: string }>;
		onprimaryaction: Effect.Effect<void>;
		policy?: ThreadSessionPolicy;
		run_active: boolean;
		runtime_catalog: RuntimeCatalog;
		send_blocked_reason?: string;
		send_ready: boolean;
	} = $props();
</script>

<div class="flex items-center justify-between gap-2">
	<ModelSelector
		{disabled}
		{engine_locked}
		{onpolicychange}
		{policy}
		{runtime_catalog}
	/>
	<div class="flex items-center gap-3">
		{#if context_usage?.context_tokens !== undefined && context_window_tokens !== undefined && context_percent !== undefined}
			<ContextUsageRing
				cached_input_tokens={context_usage.cached_input_tokens}
				context_tokens={context_usage.context_tokens}
				input_tokens={context_usage.input_tokens}
				output_tokens={context_usage.output_tokens}
				percent={context_percent}
				window_tokens={context_window_tokens}
			/>
		{/if}
		<TooltipProvider delayDuration={0}>
			<Tooltip>
				<TooltipTrigger>
					{#snippet child({ props: tooltip_props })}
						<span {...tooltip_props} class="flex has-[:disabled]:cursor-not-allowed">
							<Button
								variant="ghost"
								size="icon"
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
		:global(.composer-send)::after {
			transition: none !important;
			will-change: auto;
		}
	}
</style>
