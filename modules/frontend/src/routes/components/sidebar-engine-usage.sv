<script lang="ts" effect>
	import { Effect } from "effect";
	import { Tween } from "svelte/motion";
	import type { EngineUsageSnapshot, EngineUsageWindow } from "@artisan/protocol";
	import { DropdownMenuSeparator } from "$lib/components/ui/dropdown-menu";
	import { Skeleton } from "$lib/components/ui/skeleton";
	import { FadeArc } from "$lib/components/ui/fade-arc";
	import {
		Tooltip,
		TooltipContent,
		TooltipProvider,
		TooltipTrigger,
	} from "$lib/components/ui/tooltip";
	import { EngineMarkClass, EngineMarkFor } from "$lib/engine/presentation";
	import {
		MotionDuration,
		MotionEasing,
		RunUpFrom,
	} from "$lib/identity/usage-window-motion";
	import { weekly_reset_duration } from "$lib/identity/weekly-reset";
	import { MakeScopedAttachmentRunner } from "$lib/lifecycle/scoped-attachment-runner";
	import ShaderGlassSurface from "./shader-glass-surface.sv";
	import UsageWindowTooltip from "./usage-window-tooltip.sv";

	export type SidebarUsageState =
		| { readonly status: "idle" }
		| { readonly status: "loading" }
		| { readonly status: "loaded"; readonly snapshot: EngineUsageSnapshot }
		| { readonly status: "error" };

	const {
		checked_at_ms,
		checked_label,
		is_refreshing,
		onrefresh,
		refreshing_engines,
		usage_state,
	}: {
		readonly checked_at_ms: number;
		readonly checked_label?: string;
		readonly is_refreshing: boolean;
		readonly onrefresh: Effect.Effect<void>;
		readonly refreshing_engines: ReadonlySet<string>;
		readonly usage_state: SidebarUsageState;
	} = $props();

	const window_kind_labels: Readonly<Record<EngineUsageWindow["kind"], string>> = {
		monthly: "Monthly",
		session: "Session",
		unknown: "Usage",
		weekly: "Weekly",
	};
	const usage_segments = 14;
	const WindowLabel = (usage_window: EngineUsageWindow): string =>
		usage_window.label ?? window_kind_labels[usage_window.kind];
	const LitFraction = (percent_used: number): number =>
		Math.floor((Math.min(100, Math.max(0, percent_used)) / 100) * usage_segments) /
		usage_segments;
	/**
	 * Deduplicated by id before the keyed eachs below. The protocol does not
	 * forbid a provider reporting the same bucket twice — Claude's CLI repeats
	 * its weekly line in some layouts — and a duplicate key does not degrade,
	 * it throws, killing every engine section after the one that repeated.
	 */
	const GroupWindows = (windows: ReadonlyArray<EngineUsageWindow>) => {
		const unique = [
			...new Map(windows.map((usage_window) => [usage_window.id, usage_window])).values(),
		];
		return {
			extended: unique.filter((usage_window) => usage_window.kind !== "session"),
			session: unique.filter((usage_window) => usage_window.kind === "session"),
		};
	};

	const motion_duration = yield* MotionDuration();
	const motion_easing = yield* MotionEasing();
	const tween_options = { duration: motion_duration, easing: motion_easing };
	const remaining_reading = new Tween(0, tween_options);
	let has_read_a_window = false;
	const window_reads = yield* MakeScopedAttachmentRunner((usage_window: EngineUsageWindow) =>
		Effect.gen(function* () {
			yield* ReadWindow(usage_window);
			yield* Effect.never;
		}),
	);
	const ReadWindow = (usage_window: EngineUsageWindow) =>
		Effect.gen(function* () {
		const remaining_target = Math.max(0, 100 - Math.round(usage_window.percent_used));
		if (!has_read_a_window) {
			has_read_a_window = true;
			remaining_reading.set(RunUpFrom(remaining_target), { duration: 0 });
		}
		remaining_reading.target = remaining_target;
		});

	const authenticated_engines = $derived(
		usage_state.status === "loaded"
			? usage_state.snapshot.engines.filter(
					(engine) => engine.authentication === "authenticated",
				)
			: [],
	);
	const unavailable_engines = $derived(
		usage_state.status === "loaded"
			? usage_state.snapshot.engines.filter(
					(engine) => engine.authentication === "unknown" && engine.failure !== undefined,
				)
			: [],
	);
</script>

{#snippet usage_window(usage_entry, accent)}
	<Tooltip
		onOpenChange={(is_open) => {
			if (is_open)
				window_reads.ReplaceUnsafe(`usage-window:${usage_entry.kind}`, usage_entry);
		}}
	>
		<TooltipTrigger>
			{#snippet child({ props: tooltip_props })}
				<span {...tooltip_props} class="flex items-center gap-4 outline-none">
					<span class="min-w-0 flex-1 truncate text-xs text-muted-foreground">
						{WindowLabel(usage_entry)}
					</span>
					<span
						class="t-usage-meter h-2 w-18 min-w-18 shrink-0"
						style={`--meter-accent: ${accent}; --meter-lit: ${LitFraction(usage_entry.percent_used) * 100}%; --meter-ticks: ${usage_segments}`}
					></span>
				</span>
			{/snippet}
		</TooltipTrigger>
		<!--
			The same glass every other floating surface wears, so a reading that
			opens beside the menu belongs to it. The tooltip's own solid fill,
			padding and ring are stripped so the surface is the only thing painted;
			the caret goes with them, since a rotated square of solid fill cannot
			continue glass.
		-->
		<TooltipContent
			arrow={false}
			side="right"
			sideOffset={8}
			class="block max-w-56 rounded-2xl bg-transparent! p-0! text-foreground! shadow-none! ring-0!"
		>
			<ShaderGlassSurface class="w-full rounded-2xl" use_rays={false}>
				<span class="block px-3 py-2 text-xs text-muted-foreground">
					<UsageWindowTooltip remaining={remaining_reading.current} />
				</span>
			</ShaderGlassSurface>
		</TooltipContent>
	</Tooltip>
{/snippet}

{#if usage_state.status === "idle" || usage_state.status === "loading"}
	<div class="flex flex-col gap-2 p-2">
		<Skeleton class="h-3 w-28" />
		<Skeleton class="h-1.5 w-full" />
		<Skeleton class="h-3 w-20" />
		<Skeleton class="h-1.5 w-full" />
	</div>
{:else if usage_state.status === "error"}
	<p class="px-3 py-2.5 text-xs text-muted-foreground">Usage is unavailable right now.</p>
{:else if authenticated_engines.length === 0 && unavailable_engines.length === 0}
	<p class="px-3 py-2.5 text-xs text-muted-foreground">No engine accounts connected.</p>
{:else}
	<TooltipProvider delayDuration={0}>
		<div class="flex flex-col px-1 py-1">
			{#each authenticated_engines as engine, engine_index (engine.engine_id)}
				{@const mark = EngineMarkFor(engine.engine_id)}
				{@const MarkIcon = mark.icon}
				{@const groups = GroupWindows(engine.windows)}
				{@const weekly_reset = weekly_reset_duration(engine.windows, checked_at_ms)}
				{#if engine_index > 0}<DropdownMenuSeparator class="my-1" />{/if}
				<div class="flex flex-col gap-1.5 px-2 py-1">
					<div class="flex items-center justify-between gap-2">
						<div class="flex min-w-0 items-center gap-2">
							<MarkIcon class={EngineMarkClass(mark, "size-4")} />
							<span class="truncate text-xs font-medium text-foreground">{engine.display_name}</span>
						</div>
						{#if checked_label !== undefined}
							{@const engine_refreshing = refreshing_engines.has(engine.engine_id)}
							<button
								type="button"
								class="t-checked shrink-0 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
								data-loading={engine_refreshing}
								disabled={is_refreshing}
								onclick={yield* onrefresh}
							>
								<span class="t-checked-reading whitespace-nowrap text-muted-foreground">{checked_label}</span>
								<span class="t-checked-action whitespace-nowrap text-foreground" aria-hidden="true">Refresh</span>
								<span class="t-checked-loading" aria-hidden={!engine_refreshing}><FadeArc class="size-3.5 text-muted-foreground" /></span>
							</button>
						{/if}
					</div>
					{#if groups.session.length > 0}
						<div class="flex flex-col gap-1.5">
							{#each groups.session as usage_entry (usage_entry.id)}{@render usage_window(usage_entry, mark.accent)}{/each}
						</div>
					{/if}
					{#if groups.extended.length > 0}
						<div class="flex flex-col gap-1.5" class:mt-2={groups.session.length > 0}>
							{#each groups.extended as usage_entry (usage_entry.id)}{@render usage_window(usage_entry, mark.accent)}{/each}
						</div>
					{/if}
					{#if weekly_reset !== undefined}
						<span class="mt-2 text-xs text-muted-foreground">Your weekly limit resets in <span class="text-foreground">{weekly_reset}</span>.</span>
					{/if}
				</div>
			{/each}

			{#each unavailable_engines as engine, engine_index (engine.engine_id)}
				{@const mark = EngineMarkFor(engine.engine_id)}
				{@const MarkIcon = mark.icon}
				{#if authenticated_engines.length > 0 || engine_index > 0}<DropdownMenuSeparator class="my-1" />{/if}
				<div class="flex flex-col gap-1 px-2 py-1">
					<div class="flex items-center gap-2">
						<MarkIcon class={EngineMarkClass(mark, "size-4")} />
						<span class="truncate text-xs text-muted-foreground">{engine.display_name} — usage unavailable</span>
					</div>
					{#if engine.failure !== undefined}<p class="text-xs text-muted-foreground/70">{engine.failure}</p>{/if}
				</div>
			{/each}
		</div>
	</TooltipProvider>
{/if}

<style>
	.t-usage-meter {
		--meter-dim: color-mix(in oklab, var(--foreground) 11%, transparent);
		--meter-pitch: calc(100% / var(--meter-ticks));
		background-image: linear-gradient(to right, var(--meter-accent) 0 var(--meter-lit), var(--meter-dim) var(--meter-lit) 100%);
		mask-image: repeating-linear-gradient(to right, #000 0 calc(var(--meter-pitch) - 2px), transparent calc(var(--meter-pitch) - 2px) var(--meter-pitch));
		-webkit-mask-image: repeating-linear-gradient(to right, #000 0 calc(var(--meter-pitch) - 2px), transparent calc(var(--meter-pitch) - 2px) var(--meter-pitch));
	}
	.t-checked { display: grid; align-items: center; justify-items: end; }
	.t-checked > span {
		grid-area: 1 / 1;
		transition: opacity var(--text-swap-dur) var(--ease-in-out), filter var(--text-swap-dur) var(--ease-in-out), transform var(--text-swap-dur) var(--ease-in-out);
		will-change: opacity, filter, transform;
	}
	.t-checked .t-checked-action, .t-checked .t-checked-loading { opacity: 0; filter: blur(var(--text-swap-blur)); transform: translateY(var(--text-swap-translate-y)); }
	.t-checked:not([data-loading="true"]):hover .t-checked-reading, .t-checked:not([data-loading="true"]):focus-visible .t-checked-reading,
	.t-checked[data-loading="true"] .t-checked-reading, .t-checked[data-loading="true"] .t-checked-action { opacity: 0; filter: blur(var(--text-swap-blur)); transform: translateY(calc(-1 * var(--text-swap-translate-y))); }
	.t-checked:not([data-loading="true"]):hover .t-checked-action, .t-checked:not([data-loading="true"]):focus-visible .t-checked-action, .t-checked[data-loading="true"] .t-checked-loading { opacity: 1; filter: blur(0); transform: translateY(0); }
	@media (prefers-reduced-motion: reduce) { .t-checked > span { transition: none !important; } }
</style>
