<script lang="ts" effect>
	import Settings from "@tabler/icons-svelte/icons/settings";
	import { Effect, Option, Queue } from "effect";
	import { Tween } from "svelte/motion";
	import type { EngineUsageSnapshot, EngineUsageWindow, HostIdentitySnapshot } from "@artisan/protocol";
	import { ArtisanClient } from "@artisan/transport/client";
	import { Avatar, AvatarFallback } from "$lib/components/ui/avatar";
	import {
		DropdownMenu,
		DropdownMenuContent,
		DropdownMenuItem,
		DropdownMenuSeparator,
		DropdownMenuTrigger,
	} from "$lib/components/ui/dropdown-menu";
	import { Skeleton } from "$lib/components/ui/skeleton";
	import { FadeArc } from "$lib/components/ui/fade-arc";
	import { cn } from "$lib/utils";
	import {
		Tooltip,
		TooltipContent,
		TooltipProvider,
		TooltipTrigger,
	} from "$lib/components/ui/tooltip";
	import UsageWindowTooltip from "./usage-window-tooltip.sv";
	import { EngineMarkClass, EngineMarkFor } from "$lib/engine/presentation";
	import { GradientAvatarSvg } from "$lib/identity/gradient-avatar";
	import { model_manifest } from "@artisan/catalog";
	import { EngineUsageCache, EngineUsageCacheBrowserLive } from "$lib/identity/usage-cache";
	import {
		MotionDuration,
		MotionEasing,
		ResetPartsFor,
		RunUpFrom,
	} from "$lib/identity/usage-window-motion";

	type UsageState =
		| { readonly status: "idle" }
		| { readonly status: "loading" }
		| { readonly status: "loaded"; readonly snapshot: EngineUsageSnapshot }
		| { readonly status: "error" };

	interface UsageRequest {
		readonly engine_id: string;
		readonly force: boolean;
	}

	/**
	 * Keep the type-only request shape outside the transformed `yield*`
	 * expression. The effect-aware Svelte compiler tracks identifiers in that
	 * expression as runtime dependencies, including inline type member names.
	 */
	const MakeUsageRequests = Queue.unbounded<UsageRequest>();

	const window_kind_labels: Readonly<Record<EngineUsageWindow["kind"], string>> = {
		monthly: "Monthly",
		session: "Session",
		unknown: "Usage",
		weekly: "Weekly",
	};

	/** Ticks in one meter; 14 keeps each segment legible at the panel's 72px track. */
	const usage_segments = 14;

	const WindowLabel = (window: EngineUsageWindow): string =>
		window.label ?? window_kind_labels[window.kind];

	/**
	 * The meter is quantised, so the fill snaps to whole ticks. Reporting the raw
	 * percentage next to a meter that rounds it down would contradict itself.
	 */
	const LitFraction = (percent_used: number): number =>
		Math.floor((Math.min(100, Math.max(0, percent_used)) / 100) * usage_segments) /
		usage_segments;

	/**
	 * One tween pair for the whole panel, not one per row. Rows retarget it as they
	 * are hovered, so crossing from a 35% window to a 55% one travels between the
	 * two readings instead of each tooltip counting up from its own start.
	 */
	const tween_options = { duration: MotionDuration(), easing: MotionEasing() };
	const remaining_reading = new Tween(0, tween_options);
	const reset_reading = new Tween(0, tween_options);

	/** Not template-reactive: they only gate how the next retarget is seeded. */
	let has_read_a_window = false;
	let last_reset_unit: string | undefined = undefined;

	const ReadWindow = (window: EngineUsageWindow): void => {
		const reset = window.resets_at === undefined ? undefined : ResetPartsFor(window.resets_at);
		const remaining_target = Math.max(0, 100 - Math.round(window.percent_used));
		const reset_target = reset?.amount ?? 0;

		if (!has_read_a_window) {
			has_read_a_window = true;
			remaining_reading.set(RunUpFrom(remaining_target), { duration: 0 });
			reset_reading.set(RunUpFrom(reset_target), { duration: 0 });
		} else if (reset?.unit !== last_reset_unit) {
			/** Travelling 2 → 7 while the unit flips `h` to `d` would read as a wrong figure. */
			reset_reading.set(RunUpFrom(reset_target), { duration: 0 });
		}

		last_reset_unit = reset?.unit;
		remaining_reading.target = remaining_target;
		reset_reading.target = reset_target;
	};

	/**
	 * Splits an engine's windows so the 5-hour session limits — the ones that
	 * actually gate the next prompt — lead the list, set apart from the longer
	 * windows below them. Provider order is preserved within each group.
	 */
	const GroupWindows = (
		windows: ReadonlyArray<EngineUsageWindow>,
	): {
		readonly session: ReadonlyArray<EngineUsageWindow>;
		readonly extended: ReadonlyArray<EngineUsageWindow>;
	} => ({
		session: windows.filter((window) => window.kind === "session"),
		extended: windows.filter((window) => window.kind !== "session"),
	});

	const client = yield* ArtisanClient;
	const usage_cache = yield* EngineUsageCache.pipe(Effect.provide(EngineUsageCacheBrowserLive));

	let identity = $state<HostIdentitySnapshot | undefined>(undefined);
	let open = $state(false);
	let usage_state = $state<UsageState>({ status: "idle" });
	/** Guards the once-per-session fresh fetch; not template-reactive. */
	let has_requested_fresh_usage = false;

	const profile_name = $derived(identity?.display_name ?? identity?.username ?? identity?.hostname);
	/** The machine, not the person: one host keeps one avatar whoever is signed in. */
	const avatar_svg = $derived(
		identity === undefined
			? undefined
			: GradientAvatarSvg(identity.hostname, profile_name ?? identity.hostname),
	);
	const show_hostname = $derived(
		identity !== undefined && profile_name !== undefined && identity.hostname !== profile_name,
	);
	const authenticated_engines = $derived(
		usage_state.status === "loaded"
			? usage_state.snapshot.engines.filter((engine) => engine.authentication === "authenticated")
			: [],
	);
	/** Engines with a fetch in flight; each header shows its own arc while listed here. */
	let refreshing_engines = $state<ReadonlySet<string>>(new Set());
	const is_refreshing = $derived(refreshing_engines.size > 0);
	const unavailable_engines = $derived(
		usage_state.status === "loaded"
			? usage_state.snapshot.engines.filter(
					(engine) => engine.authentication === "unknown" && engine.failure !== undefined,
				)
			: [],
	);

	/**
	 * Every harness the catalog knows. Engines without a usage surface answer
	 * with an empty report at protocol speed, so over-asking costs nothing and
	 * keeps this list free of a second round trip to discover engine ids.
	 */
	const usage_engine_ids = model_manifest.harnesses.map((harness) => harness.id);

	/** Queued open requests fork the (potentially multi-second) usage fetches exactly once per session. */
	const usage_requests = yield* MakeUsageRequests;

	/** Upserts one engine's reports so each provider paints as soon as it answers. */
	const MergeReports = (incoming: EngineUsageSnapshot) => {
		const prior = usage_state.status === "loaded" ? usage_state.snapshot.engines : [];
		const by_id = new Map(prior.map((report) => [report.engine_id, report] as const));
		for (const report of incoming.engines) by_id.set(report.engine_id, report);
		usage_state = {
			snapshot: { engines: [...by_id.values()], fetched_at: incoming.fetched_at },
			status: "loaded",
		};
	};

	const FetchEngineUsage = (engine_id: string, force: boolean) =>
		client.GetEngineUsage({ engine_id, ...(force ? { force: true } : {}) }).pipe(
			Effect.tap((snapshot) =>
				Effect.sync(() => {
					if (snapshot.engines.length > 0) MergeReports(snapshot);
				}),
			),
			Effect.catch(() => Effect.void),
			Effect.ensuring(
				Effect.gen(function* () {
					const remaining = new Set(refreshing_engines);
					remaining.delete(engine_id);
					refreshing_engines = remaining;
					if (remaining.size > 0) return;
					/** The fan-out has drained: settle the cache, or the error state when nothing ever loaded. */
					if (usage_state.status === "loaded") {
						yield* usage_cache
							.Save(usage_state.snapshot)
							.pipe(Effect.catch(() => Effect.void));
					} else if (usage_state.status === "loading") {
						usage_state = { status: "error" };
					}
				}),
			),
		);

	/**
	 * Fans one fetch out per engine; the manual control and the auto-open path
	 * share it. Manual refreshes force past the backend's freshness window, so
	 * the providers are genuinely re-asked; the automatic session fetch accepts
	 * backend-cached reports. Results merge in as each provider answers rather
	 * than waiting for the slowest one.
	 */
	const RefreshUsage = (force: boolean) => {
		if (usage_state.status === "loading" || is_refreshing) return;

		refreshing_engines = new Set(usage_engine_ids);
		if (usage_state.status !== "loaded") usage_state = { status: "loading" };
		for (const engine_id of usage_engine_ids) {
			Queue.offerUnsafe(usage_requests, { engine_id, force });
		}
	};

	const RequestUsage = () => {
		if (has_requested_fresh_usage) return;
		has_requested_fresh_usage = true;
		RefreshUsage(false);
	};

	/**
	 * The "last checked" reading ages on a coarse scoped clock rather than
	 * freezing at the value from the last menu open.
	 */
	let checked_at_ms = $state(Date.now());
	yield* Effect.sleep("30 seconds").pipe(
		Effect.andThen(
			Effect.sync(() => {
				checked_at_ms = Date.now();
			}),
		),
		Effect.forever,
		Effect.forkScoped,
	);

	const FormatCheckedLabel = (fetched_at: string, at_ms: number): string => {
		const minutes = Math.floor((at_ms - Date.parse(fetched_at)) / 60_000);
		if (minutes < 1) return "last checked now";
		if (minutes < 60) return `last checked ${minutes} min ago`;
		const hours = Math.floor(minutes / 60);
		if (hours < 24) return `last checked ${hours} hr ago`;
		return `last checked ${Math.floor(hours / 24)} d ago`;
	};

	const checked_label = $derived(
		usage_state.status === "loaded"
			? FormatCheckedLabel(usage_state.snapshot.fetched_at, checked_at_ms)
			: undefined,
	);

	yield* client.GetHostIdentity.pipe(
		Effect.tap((snapshot) =>
			Effect.sync(() => {
				identity = snapshot;
			}),
		),
		Effect.catch(() => Effect.void),
	);

	const cached_usage = yield* usage_cache.Load;
	if (Option.isSome(cached_usage)) {
		usage_state = { status: "loaded", snapshot: cached_usage.value };
	}

	yield* Queue.take(usage_requests).pipe(
		Effect.flatMap((request) =>
			Effect.forkScoped(FetchEngineUsage(request.engine_id, request.force)),
		),
		Effect.forever,
		Effect.forkScoped,
	);

	$effect(() => {
		if (open) RequestUsage();
	});
</script>

{#snippet usage_window(window: EngineUsageWindow, accent: string)}
	<Tooltip
		onOpenChange={(is_open) => {
			if (is_open) ReadWindow(window);
		}}
	>
		<TooltipTrigger>
			{#snippet child({ props: tooltip_props })}
				<!--
					A usage row is static text, not a control. Left focusable, the menu's
					open-focus lands on the first one and springs its tooltip unprompted.
				-->
					<span {...tooltip_props} class="flex items-center gap-4 outline-none">
					<span class="min-w-0 flex-1 truncate text-xs text-muted-foreground">
							{WindowLabel(window)}
						</span>
					<span
						class="t-usage-meter h-2 w-18 min-w-18 shrink-0"
						style={`--meter-accent: ${accent}; --meter-lit: ${LitFraction(window.percent_used) * 100}%; --meter-ticks: ${usage_segments}`}
					></span>
				</span>
			{/snippet}
		</TooltipTrigger>
		<TooltipContent side="right" class="max-w-56">
			<UsageWindowTooltip
				amount={reset_reading.current}
				remaining={remaining_reading.current}
				reset={window.resets_at === undefined ? undefined : ResetPartsFor(window.resets_at)}
			/>
		</TooltipContent>
	</Tooltip>
{/snippet}

<DropdownMenu bind:open>
	<!--
		The trigger is the avatar and nothing else — no card, no fill. The avatar
		already reads as a control at that size, and a housing around it would
		compete with the rail's pill above it. The name and hostname live in the
		menu it opens, not on the rail.
	-->
	<DropdownMenuTrigger
		aria-label="Account"
		class="account-trigger mx-auto flex size-10 items-center justify-center rounded-full p-0 outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
	>
		<Avatar class="account-avatar inset-shadow-artwork">
			{#if avatar_svg === undefined}
				<AvatarFallback />
			{:else}
				<span class="flex size-full">
					<!-- eslint-disable-next-line svelte/no-at-html-tags -- generated markup, no user input -->
					{@html avatar_svg}
				</span>
			{/if}
		</Avatar>
	</DropdownMenuTrigger>

	<DropdownMenuContent side="top" align="start" class="w-auto min-w-64 max-w-88">
		<div class="flex min-w-0 flex-row items-center gap-3 px-3 py-4">
			<Avatar class="size-8 inset-shadow-artwork">
				{#if avatar_svg === undefined}
					<AvatarFallback />
				{:else}
					<span class="flex size-full">
						<!-- eslint-disable-next-line svelte/no-at-html-tags -- generated markup, no user input -->
						{@html avatar_svg}
					</span>
				{/if}
			</Avatar>
			<div class="flex min-w-0 flex-col -space-y-1">
				<span class="truncate text-sm font-medium text-foreground">
					{profile_name ?? "Not connected"}
				</span>
				{#if show_hostname}
					<span class="truncate text-xs text-muted-foreground">{identity?.hostname}</span>
				{/if}
			</div>
		</div>

		<DropdownMenuSeparator />

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
			<div class="flex flex-col gap-2.5 px-1 py-1">
				{#each authenticated_engines as engine (engine.engine_id)}
					{@const mark = EngineMarkFor(engine.engine_id)}
					{@const MarkIcon = mark.icon}
					{@const groups = GroupWindows(engine.windows)}
					<div class="flex flex-col gap-1.5 px-2 py-1">
						<div class="flex items-center justify-between gap-2">
							<div class="flex min-w-0 items-center gap-2">
								<MarkIcon class={EngineMarkClass(mark, "size-4")} />
								<span class="truncate text-xs font-medium text-foreground">
									{engine.display_name}
								</span>
							</div>
							{#if checked_label !== undefined}
								{@const engine_refreshing = refreshing_engines.has(engine.engine_id)}
								<!--
									One slot, three states, all stacked in the same grid cell and
									swapped in place: the reading at rest, Refresh on hover, and
									the arc while this engine's own fetch is in flight — each
									provider's arc clears as soon as it answers.
								-->
								<button
									type="button"
									class="t-checked shrink-0 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
									data-loading={engine_refreshing}
									disabled={is_refreshing}
									onclick={() => RefreshUsage(true)}
								>
									<span class="t-checked-reading whitespace-nowrap text-muted-foreground">
										{checked_label}
									</span>
									<span class="t-checked-action whitespace-nowrap text-foreground" aria-hidden="true">
										Refresh
									</span>
									<span class="t-checked-loading" aria-hidden={!engine_refreshing}>
										<FadeArc class="size-3.5 text-muted-foreground" />
									</span>
								</button>
							{/if}
						</div>
						{#if groups.session.length > 0}
							<div class="flex flex-col gap-1.5">
								{#each groups.session as window (window.id)}
									{@render usage_window(window, mark.accent)}
								{/each}
							</div>
						{/if}
						{#if groups.extended.length > 0}
							<div class="flex flex-col gap-1.5" class:mt-2={groups.session.length > 0}>
								{#each groups.extended as window (window.id)}
									{@render usage_window(window, mark.accent)}
								{/each}
							</div>
						{/if}
					</div>
				{/each}

				{#each unavailable_engines as engine (engine.engine_id)}
					{@const mark = EngineMarkFor(engine.engine_id)}
					{@const MarkIcon = mark.icon}
					<div class="flex flex-col gap-1 px-2 py-1">
						<div class="flex items-center gap-2">
							<MarkIcon class={EngineMarkClass(mark, "size-4")} />
							<span class="truncate text-xs text-muted-foreground">
								{engine.display_name} — usage unavailable
							</span>
						</div>
						{#if engine.failure !== undefined}
							<p class="text-xs text-muted-foreground/70">{engine.failure}</p>
						{/if}
					</div>
				{/each}
			</div>
			</TooltipProvider>
		{/if}

		<DropdownMenuSeparator />

		<DropdownMenuItem>
			<Settings class="size-4 shrink-0 text-muted-foreground" />
			Settings
		</DropdownMenuItem>
	</DropdownMenuContent>
</DropdownMenu>

<style>
	/**
	 * Hover feedback is a shimmer across the avatar artwork rather than an
	 * accent fill — shimmer-text's sweeping band, painted on an overlay that
	 * ramps from transparent instead of clipping to glyphs. The band travels
	 * by transform rather than background-position so its resting point can
	 * sit exactly off the top-right edge: the sweep is visible from the first
	 * frame of the hover instead of spending part of the cycle off-canvas.
	 * The avatar's overflow clips the pass to the circle.
	 */
	:global(.account-avatar)::after {
		--account-shimmer-contrast: rgb(255 255 255 / 40%);
		content: "";
		position: absolute;
		inset: 0;
		pointer-events: none;
		background-image: linear-gradient(
			to bottom left,
			transparent 35%,
			var(--account-shimmer-contrast) 45%,
			var(--account-shimmer-contrast) 55%,
			transparent 65%
		);
		transform: translate(100%, -100%);
		will-change: transform;
	}
	/**
	 * One pass per hover, not a loop: the keyframes end off the bottom-left
	 * edge and the resting transform is off the top-right, so after the pass
	 * the overlay is invisible either way. Leaving and re-entering re-applies
	 * the animation and replays it.
	 */
	:global(.account-trigger:hover .account-avatar)::after {
		animation: account-shimmer 600ms var(--ease-smooth-out);
	}

	@keyframes -global-account-shimmer {
		from {
			transform: translate(100%, -100%);
		}
		to {
			transform: translate(-100%, 100%);
		}
	}

	@media (prefers-reduced-motion: reduce) {
		:global(.account-trigger:hover .account-avatar)::after {
			animation: none !important;
		}
	}

	/**
	 * One element rather than N tick spans: the ticks are a repeating mask over a
	 * single fill gradient, so a row costs one node and one paint.
	 */
	.t-usage-meter {
		--meter-dim: color-mix(in oklab, var(--foreground) 11%, transparent);
		--meter-pitch: calc(100% / var(--meter-ticks));
		background-image: linear-gradient(
			to right,
			var(--meter-accent) 0 var(--meter-lit),
			var(--meter-dim) var(--meter-lit) 100%
		);
		mask-image: repeating-linear-gradient(
			to right,
			#000 0 calc(var(--meter-pitch) - 2px),
			transparent calc(var(--meter-pitch) - 2px) var(--meter-pitch)
		);
		-webkit-mask-image: repeating-linear-gradient(
			to right,
			#000 0 calc(var(--meter-pitch) - 2px),
			transparent calc(var(--meter-pitch) - 2px) var(--meter-pitch)
		);
	}

	/**
	 * Text states swap (transitions.dev) across the slot's three states: the
	 * reading at rest, "Refresh" on hover, and the fade arc while fetching.
	 * All three share one grid cell; whichever leaves slides up and out
	 * blurred while the newcomer rises from below on the text-swap tokens.
	 */
	.t-checked {
		display: grid;
		align-items: center;
		justify-items: end;
	}

	.t-checked > span {
		grid-area: 1 / 1;
		transition:
			opacity var(--text-swap-dur) var(--ease-in-out),
			filter var(--text-swap-dur) var(--ease-in-out),
			transform var(--text-swap-dur) var(--ease-in-out);
		will-change: opacity, filter, transform;
	}

	.t-checked .t-checked-action,
	.t-checked .t-checked-loading {
		opacity: 0;
		filter: blur(var(--text-swap-blur));
		transform: translateY(var(--text-swap-translate-y));
	}

	.t-checked:not([data-loading="true"]):hover .t-checked-reading,
	.t-checked:not([data-loading="true"]):focus-visible .t-checked-reading {
		opacity: 0;
		filter: blur(var(--text-swap-blur));
		transform: translateY(calc(-1 * var(--text-swap-translate-y)));
	}

	.t-checked:not([data-loading="true"]):hover .t-checked-action,
	.t-checked:not([data-loading="true"]):focus-visible .t-checked-action {
		opacity: 1;
		filter: blur(0);
		transform: translateY(0);
	}

	/* Fetching takes the slot from whichever text state was holding it. */
	.t-checked[data-loading="true"] .t-checked-reading,
	.t-checked[data-loading="true"] .t-checked-action {
		opacity: 0;
		filter: blur(var(--text-swap-blur));
		transform: translateY(calc(-1 * var(--text-swap-translate-y)));
	}

	.t-checked[data-loading="true"] .t-checked-loading {
		opacity: 1;
		filter: blur(0);
		transform: translateY(0);
	}

	@media (prefers-reduced-motion: reduce) {
		.t-checked > span {
			transition: none !important;
		}
	}
</style>
