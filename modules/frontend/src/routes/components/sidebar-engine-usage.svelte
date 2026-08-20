<script lang="ts" effect>
	import { Effect } from "effect";
	import { Tween } from "svelte/motion";
	import type {
		EngineUsageReport,
		EngineUsageSnapshot,
		EngineUsageWindow,
	} from "@artisan/protocol";
	import { DropdownMenuSeparator } from "$lib/components/ui/dropdown-menu";
	import { Skeleton } from "$lib/components/ui/skeleton";
	import { FadeArc } from "$lib/components/ui/fade-arc";
	import {
		Tooltip,
		TooltipContent,
		TooltipProvider,
		TooltipTrigger,
	} from "$lib/components/ui/tooltip";
	import { EngineDisplayName, EngineMarkClass, EngineMarkFor } from "$lib/engine/presentation";
	import type { EngineUsageEntry } from "$lib/identity/engine-usage-controller";
	import {
		usage_meter_segments,
		usage_segment_fraction,
	} from "$lib/identity/usage-meter";
	import {
		MotionDuration,
		MotionEasing,
		RunUpFrom,
	} from "$lib/identity/usage-window-motion";
	import { weekly_reset_duration } from "$lib/identity/weekly-reset";
	import { MakeScopedAttachmentRunner } from "$lib/lifecycle/scoped-attachment-runner";
	import ShaderGlassSurface from "./shader-glass-surface.svelte";
	import UsageWindowTooltip from "./usage-window-tooltip.svelte";

	export type SidebarUsageState =
		| { readonly status: "idle" }
		| { readonly status: "loading" }
		| { readonly status: "loaded"; readonly snapshot: EngineUsageSnapshot }
		| { readonly status: "error" };

	const {
		checked_at_ms,
		entries,
		engine_ids,
		onrefresh,
		refreshing_engines,
		usage_state,
		hidden_engine_ids,
	}: {
		readonly checked_at_ms: number;
		/**
		 * Per-engine readings, each with its own timestamp and its own failure.
		 *
		 * The rows used to share one aggregate: a single "last checked" taken from
		 * the newest engine, printed on every row including engines whose reading
		 * was hours older, and a single error status that replaced the whole list
		 * when nothing had answered yet. Each row now states only what its own
		 * engine reported.
		 */
		readonly entries: ReadonlyMap<string, EngineUsageEntry>;
		/**
		 * Enabled engines in catalog order. The row set and its ordering come
		 * from this list — known before any provider answers — never from which
		 * provider happened to answer first, so every enabled engine has a named
		 * row from the first frame and fills in place.
		 */
		readonly engine_ids: ReadonlyArray<string>;
		/** Called from the row's direct SER event site with that provider's id. */
		readonly onrefresh: (engine_id: string) => Effect.Effect<void>;
		readonly refreshing_engines: ReadonlySet<string>;
		readonly usage_state: SidebarUsageState;
		/**
		 * Engines the user switched off. A cached snapshot can still carry their
		 * last report, and a switched-off engine must not be resurrected by its
		 * own cache entry.
		 */
		readonly hidden_engine_ids?: ReadonlyArray<string>;
	} = $props();

	const window_kind_labels: Readonly<Record<EngineUsageWindow["kind"], string>> = {
		monthly: "Monthly",
		session: "Session",
		unknown: "Usage",
		weekly: "Weekly",
	};
	const WindowLabel = (usage_window: EngineUsageWindow): string =>
		usage_window.label ?? window_kind_labels[usage_window.kind];
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

	/**
	 * One row per enabled engine, in catalog order, resolved against whatever
	 * report the snapshot holds right now. The whole set is known before any
	 * provider answers, so the menu never passes through an anonymous-skeleton
	 * phase that later re-resolves into named sections: every engine paints
	 * its mark and name immediately — skeleton meters until its first report
	 * lands ("pending") — and fills in place as its provider answers. An
	 * engine that already has a report keeps painting that report while it
	 * refreshes; a cleanly signed-out engine remains visible with an explicit
	 * authentication state.
	 */
	type EngineUsageRow =
		| {
				readonly engine_id: string;
				readonly kind: "authenticated" | "unavailable";
				readonly report: EngineUsageReport;
		  }
		| {
				readonly engine_id: string;
				readonly kind: "unauthenticated";
				readonly report?: EngineUsageReport;
		  }
		| { readonly engine_id: string; readonly kind: "pending"; readonly report?: undefined };

	/**
	 * How long ago this engine answered, in its own words. Absent until it has
	 * answered at least once, which is also what withholds its refresh control.
	 */
	const CheckedLabel = (engine_id: string): string | undefined => {
		const fetched_at_ms = entries.get(engine_id)?.fetched_at_ms;
		if (fetched_at_ms === undefined || !Number.isFinite(fetched_at_ms)) return undefined;
		const minutes = Math.floor((checked_at_ms - fetched_at_ms) / 60_000);
		if (minutes < 1) return "last checked now";
		if (minutes < 60) return `last checked ${minutes} min ago`;
		const hours = Math.floor(minutes / 60);
		if (hours < 24) return `last checked ${hours} hr ago`;
		return `last checked ${Math.floor(hours / 24)} d ago`;
	};

	const RowFor = (engine_id: string): EngineUsageRow | undefined => {
		const entry = entries.get(engine_id);
		const report = entry?.report;
		if (report !== undefined) {
			if (report.authentication === "authenticated") {
				return { engine_id, kind: "authenticated", report };
			}
			return report.authentication === "unauthenticated"
				? { engine_id, kind: "unauthenticated", report }
				: { engine_id, kind: "unavailable", report };
		}
		/** A recorded failure is this engine's own answer, not a missing one. */
		if (entry?.failure !== undefined) {
			return {
				engine_id,
				kind: "unavailable",
				report: {
					authentication: "unknown",
					display_name: EngineDisplayName(engine_id),
					engine_id,
					failure: entry.failure,
					windows: [],
				},
			};
		}
		return refreshing_engines.has(engine_id)
			? { engine_id, kind: "pending" }
			: { engine_id, kind: "unauthenticated" };
	};
	const engine_rows = $derived(
		engine_ids
			.filter((engine_id) => !(hidden_engine_ids ?? []).includes(engine_id))
			.map(RowFor)
			.filter((row): row is EngineUsageRow => row !== undefined),
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
						style={`--meter-accent: ${accent}; --meter-lit: ${usage_segment_fraction(usage_entry.percent_used) * 100}%; --meter-ticks: ${usage_meter_segments}`}
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

<!--
	No aggregate failure branch: one engine failing is that engine's row saying
	so, never a message that replaces the engines that did answer.
-->
{#if engine_rows.length === 0}
	{#if usage_state.status === "loaded"}
		<p class="px-3 py-2.5 text-xs text-muted-foreground">No engine accounts connected.</p>
	{:else}
		<!--
			Reachable only in the instant before the session's first fan-out when
			no snapshot is cached — once the fan-out starts, every enabled engine
			has a named row below instead of this anonymous shimmer.
		-->
		<div class="flex flex-col gap-2 p-2">
			<Skeleton class="h-3 w-28" />
			<Skeleton class="h-1.5 w-full" />
			<Skeleton class="h-3 w-20" />
			<Skeleton class="h-1.5 w-full" />
		</div>
	{/if}
{:else}
	<TooltipProvider delayDuration={0}>
		<div class="flex flex-col px-1 py-1">
			{#each engine_rows as row, row_index (row.engine_id)}
				{@const mark = EngineMarkFor(row.engine_id)}
				{@const MarkIcon = mark.icon}
				{#if row_index > 0}<DropdownMenuSeparator class="my-1" />{/if}
				{#if row.kind === "authenticated"}
					{@const engine = row.report}
					{@const groups = GroupWindows(engine.windows)}
					{@const weekly_reset = weekly_reset_duration(engine.windows, checked_at_ms)}
					{@const engine_checked_label = CheckedLabel(engine.engine_id)}
					<div class="flex flex-col gap-1.5 px-2 py-1">
						<div class="flex items-center justify-between gap-2">
							<div class="flex min-w-0 items-center gap-2">
								<MarkIcon class={EngineMarkClass(mark, "size-4")} />
								<span class="truncate text-xs font-medium text-foreground">{engine.display_name}</span>
							</div>
							{#if engine_checked_label !== undefined}
								{@const engine_refreshing = refreshing_engines.has(engine.engine_id)}
								<button
									type="button"
									class="t-checked shrink-0 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
									data-loading={engine_refreshing}
									disabled={engine_refreshing}
									onclick={yield* onrefresh(engine.engine_id)}
								>
									<span class="t-checked-reading whitespace-nowrap text-muted-foreground">{engine_checked_label}</span>
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
				{:else if row.kind === "pending"}
					<!--
						The engine's real mark and name over skeleton meters: the identity
						is known before the fan-out starts, and only the reading is
						pending. An anonymous shimmer here would say "something may
						exist", which is exactly the doubt this row exists to remove.
					-->
					<div
						class="flex flex-col gap-1.5 px-2 py-1"
						aria-label={`${EngineDisplayName(row.engine_id)} usage loading`}
					>
						<div class="flex items-center justify-between gap-2">
							<div class="flex min-w-0 items-center gap-2">
								<MarkIcon class={EngineMarkClass(mark, "size-4")} />
								<span class="truncate text-xs font-medium text-foreground">
									{EngineDisplayName(row.engine_id)}
								</span>
							</div>
							<FadeArc class="size-3.5 shrink-0 text-muted-foreground" />
						</div>
						<div class="flex flex-col gap-1.5">
							<Skeleton class="h-3 w-24" />
							<Skeleton class="h-1.5 w-full" />
							<Skeleton class="h-3 w-16" />
							<Skeleton class="h-1.5 w-full" />
						</div>
					</div>
				{:else if row.kind === "unauthenticated"}
					<div class="flex flex-col gap-1 px-2 py-1">
						<div class="flex items-center gap-2">
							<MarkIcon class={EngineMarkClass(mark, "size-4")} />
							<span class="truncate text-xs font-medium text-foreground">{EngineDisplayName(row.engine_id)}</span>
						</div>
						<p class="pl-6 text-xs text-muted-foreground">{EngineDisplayName(row.engine_id)} is not authenticated</p>
					</div>
				{:else}
					{@const engine = row.report}
					<div class="flex flex-col gap-1 px-2 py-1">
						<div class="flex items-center gap-2">
							<MarkIcon class={EngineMarkClass(mark, "size-4")} />
							<span class="truncate text-xs font-medium text-foreground">{engine.display_name}</span>
						</div>
						<p class="pl-6 text-xs text-muted-foreground">
							{engine.failure ?? "Usage is unavailable right now."}
						</p>
					</div>
				{/if}
			{/each}
		</div>
	</TooltipProvider>
{/if}
