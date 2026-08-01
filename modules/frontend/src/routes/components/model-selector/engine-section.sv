<script lang="ts" effect>
	import { Effect } from "effect";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { TabsList, TabsTrigger } from "$lib/components/ui/tabs";
	import { Tooltip, TooltipContent, TooltipTrigger } from "$lib/components/ui/tooltip";
	import type { EngineChoice } from "$lib/engine/model-selection";

	let {
		active_engine,
		disabled,
		disabled_reason,
		engine_locked,
		engines,
		selected_engine,
	}: {
		active_engine: EngineChoice["id"];
		disabled: boolean;
		disabled_reason?: string;
		engine_locked: boolean;
		engines: ReadonlyArray<EngineChoice>;
		selected_engine: EngineChoice;
	} = $props();

	let surface = $state<HTMLElement | null>(null);
	let indicator_animated = $state(false);
	let indicator_left = $state(0);
	let indicator_visible = $state(false);
	let indicator_width = $state(0);
	let resize_revision = $state(0);

	const PositionIndicator = (animate: boolean) =>
		Effect.gen(function* () {
			/**
			 * Yield one browser task so the tab primitive has committed its new
			 * active state and geometry before measuring it.
			 */
			yield* Effect.sleep("1 millis");
			const current_surface = surface;
			const active_tab = yield* RunBrowserDom(() =>
				current_surface?.querySelector<HTMLElement>(`[data-engine="${active_engine}"]`),
			);
			if (active_tab === undefined || active_tab === null || current_surface === null) return;

			const { surface_rect, tab_rect } = yield* RunBrowserDom(() => ({
				surface_rect: current_surface.getBoundingClientRect(),
				tab_rect: active_tab.getBoundingClientRect(),
			}));
			indicator_animated = animate && indicator_visible;
			indicator_left = tab_rect.left - surface_rect.left;
			indicator_visible = true;
			indicator_width = tab_rect.width;
		});

	const Resize = Effect.gen(function* () {
		resize_revision += 1;
	});

	/** SER reruns this scoped geometry program after a tab, surface, or resize change. */
	yield* PositionIndicator(resize_revision === 0);
</script>

<svelte:window onresize={yield* Resize} />

<TabsList
	bind:ref={surface}
	variant="line"
	aria-label="Coding engines"
	class="card relative h-auto! w-full justify-start overflow-x-auto rounded-lg! bg-linear-to-b from-surface-225 to-surface-200 p-1 dark:from-surface-800 dark:to-surface-925"
>
	<div
		class="engine-light"
		data-active={indicator_visible}
		data-animate={indicator_animated}
		aria-hidden="true"
		style={`--engine-light-x: ${indicator_left}px; --engine-light-width: ${indicator_width}px;`}
	></div>
	{#each engines as engine (engine.id)}
		{@const EngineIcon = engine.icon}
		{@const engine_disabled_reason =
			engine_locked && engine.id !== selected_engine.id
				? `${engine.name} — finish the active run before switching engines`
				: disabled_reason}
		<Tooltip>
			<TooltipTrigger>
				{#snippet child({ props: tooltip_props })}
					<span {...tooltip_props} class="flex flex-none has-[:disabled]:cursor-not-allowed">
						<TabsTrigger
							value={engine.id}
							disabled={disabled || (engine_locked && engine.id !== selected_engine.id)}
							data-engine={engine.id}
							aria-label={engine.name}
							class="relative z-1 size-8 flex-none px-0 text-foreground after:hidden hover:text-foreground data-active:border-transparent data-active:bg-transparent data-active:text-foreground dark:hover:text-foreground dark:data-active:border-transparent dark:data-active:bg-transparent"
						>
							<EngineIcon class={engine.monochrome ? "size-4 dark:invert" : "size-4"} />
						</TabsTrigger>
					</span>
				{/snippet}
			</TooltipTrigger>
			{#if engine_disabled_reason !== undefined}
				<TooltipContent>{engine_disabled_reason}</TooltipContent>
			{/if}
		</Tooltip>
	{/each}
</TabsList>

<style>
	.engine-light {
		position: absolute;
		top: 0;
		left: 0;
		z-index: 0;
		width: var(--engine-light-width, 0);
		height: 100%;
		pointer-events: none;
		opacity: 0;
		transform: translate3d(var(--engine-light-x, 0), 0, 0);
		will-change: transform, width;
	}

	.engine-light::after {
		position: absolute;
		top: -0.125rem;
		left: 50%;
		width: 2rem;
		height: 1.5rem;
		content: "";
		background: linear-gradient(
			to bottom,
			oklch(from var(--foreground) l c h / 32%) 0%,
			oklch(from var(--foreground) l c h / 10%) 26%,
			oklch(from var(--foreground) l c h / 2%) 52%,
			transparent 74%
		);
		filter: blur(0.125rem);
		-webkit-mask-image: radial-gradient(
			ellipse 48% 70% at 50% 35%,
			#000 0%,
			rgb(0 0 0 / 50%) 42%,
			rgb(0 0 0 / 10%) 68%,
			transparent 88%
		);
		mask-image: radial-gradient(
			ellipse 48% 70% at 50% 35%,
			#000 0%,
			rgb(0 0 0 / 50%) 42%,
			rgb(0 0 0 / 10%) 68%,
			transparent 88%
		);
		transform: translateX(-50%);
	}

	.engine-light[data-active="true"] {
		opacity: 1;
	}

	.engine-light[data-animate="true"] {
		transition:
			transform var(--duration-fast) var(--ease-smooth-out),
			width var(--duration-fast) var(--ease-smooth-out);
	}

	@media (prefers-reduced-motion: reduce) {
		.engine-light {
			transition: none !important;
		}
	}
</style>
