<script lang="ts" effect>
	import Settings from "@tabler/icons-svelte/icons/settings";
	import { Effect, Option, Queue } from "effect";
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
	import {
		Tooltip,
		TooltipContent,
		TooltipProvider,
		TooltipTrigger,
	} from "$lib/components/ui/tooltip";
	import { EngineMarkClass, EngineMarkFor } from "$lib/engine/presentation";
	import { EngineUsageCache, EngineUsageCacheBrowserLive } from "$lib/identity/usage-cache";

	type UsageState =
		| { readonly status: "idle" }
		| { readonly status: "loading" }
		| {
				readonly status: "loaded";
				readonly snapshot: EngineUsageSnapshot;
				/** A fresh fetch is in flight; keep showing `snapshot` rather than regressing to skeletons. */
				readonly refreshing: boolean;
		  }
		| { readonly status: "error" };

	const window_kind_labels: Readonly<Record<EngineUsageWindow["kind"], string>> = {
		monthly: "Monthly",
		session: "Session",
		unknown: "Usage",
		weekly: "Weekly",
	};

	/** First letters of the first two words of a name, or the leading two characters of a single word. */
	const InitialsFor = (name: string): string => {
		const words = name.trim().split(/\s+/).filter((word) => word.length > 0);
		if (words.length === 0) return "?";
		if (words.length === 1) return (words[0] ?? "").slice(0, 2).toUpperCase();
		return `${words[0]?.[0] ?? ""}${words[1]?.[0] ?? ""}`.toUpperCase();
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

	/** The row shows no number, so the tooltip carries the exact figure and the reset. */
	const UsageTooltip = (window: EngineUsageWindow): string =>
		`${Math.round(window.percent_used)}% used${
			window.resets_at === undefined ? "" : ` · resets ${RelativeReset(window.resets_at)}`
		}`;

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

	/** Renders a compact relative time (`in 3h`, `2d ago`) with no new dependency. */
	const RelativeReset = (iso: string): string => {
		const diff_ms = new Date(iso).getTime() - Date.now();
		const abs_minutes = Math.round(Math.abs(diff_ms) / 60_000);
		const suffix = diff_ms < 0 ? " ago" : "";
		const prefix = diff_ms >= 0 ? "in " : "";

		if (abs_minutes < 1) return "now";
		if (abs_minutes < 60) return `${prefix}${abs_minutes}m${suffix}`;
		if (abs_minutes < 60 * 24) return `${prefix}${Math.round(abs_minutes / 60)}h${suffix}`;
		return `${prefix}${Math.round(abs_minutes / (60 * 24))}d${suffix}`;
	};

	const client = yield* ArtisanClient;
	const usage_cache = yield* EngineUsageCache.pipe(Effect.provide(EngineUsageCacheBrowserLive));

	let identity = $state<HostIdentitySnapshot | undefined>(undefined);
	let open = $state(false);
	let usage_state = $state<UsageState>({ status: "idle" });
	/** Guards the once-per-session fresh fetch; not template-reactive. */
	let has_requested_fresh_usage = false;

	const profile_name = $derived(identity?.display_name ?? identity?.username ?? identity?.hostname);
	const initials = $derived(profile_name === undefined ? "?" : InitialsFor(profile_name));
	const show_hostname = $derived(
		identity !== undefined && profile_name !== undefined && identity.hostname !== profile_name,
	);
	const authenticated_engines = $derived(
		usage_state.status === "loaded"
			? usage_state.snapshot.engines.filter((engine) => engine.authentication === "authenticated")
			: [],
	);
	/** Drives the meter shimmer while a fresh snapshot is in flight behind cached values. */
	const is_refreshing = $derived(usage_state.status === "loaded" && usage_state.refreshing);
	const unavailable_engines = $derived(
		usage_state.status === "loaded"
			? usage_state.snapshot.engines.filter(
					(engine) => engine.authentication === "unknown" && engine.failure !== undefined,
				)
			: [],
	);

	/** Queued open requests fork the (potentially multi-second) usage fetch exactly once per session. */
	const usage_requests = yield* Queue.unbounded<void>();

	const FetchUsage = client.GetEngineUsage.pipe(
		Effect.tap((snapshot) =>
			Effect.gen(function* () {
				usage_state = { status: "loaded", snapshot, refreshing: false };
				yield* usage_cache.Save(snapshot).pipe(Effect.catch(() => Effect.void));
			}),
		),
		Effect.catch(() =>
			Effect.sync(() => {
				/** Keep cached values on screen; only regress to an error state when there was nothing cached. */
				usage_state =
					usage_state.status === "loaded"
						? { ...usage_state, refreshing: false }
						: { status: "error" };
			}),
		),
	);

	const RequestUsage = () => {
		if (has_requested_fresh_usage) return;
		has_requested_fresh_usage = true;

		usage_state =
			usage_state.status === "loaded"
				? { ...usage_state, refreshing: true }
				: { status: "loading" };

		Queue.offerUnsafe(usage_requests, undefined);
	};

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
		usage_state = { status: "loaded", snapshot: cached_usage.value, refreshing: false };
	}

	yield* Queue.take(usage_requests).pipe(
		Effect.flatMap(() => FetchUsage),
		Effect.forever,
		Effect.forkScoped,
	);

	$effect(() => {
		if (open) RequestUsage();
	});
</script>

{#snippet usage_window(window: EngineUsageWindow, accent: string, refreshing: boolean)}
	<Tooltip>
		<TooltipTrigger>
			{#snippet child({ props: tooltip_props })}
				<span {...tooltip_props} class="flex items-center gap-4">
					<span class="min-w-0 flex-1 truncate text-xs text-muted-foreground">
						{WindowLabel(window)}
					</span>
					<span
						class="t-usage-meter h-2 w-18 min-w-18 shrink-0"
						class:animate-pulse={refreshing}
						style={`--meter-accent: ${accent}; --meter-lit: ${LitFraction(window.percent_used) * 100}%; --meter-ticks: ${usage_segments}`}
					></span>
				</span>
			{/snippet}
		</TooltipTrigger>
		<TooltipContent>{UsageTooltip(window)}</TooltipContent>
	</Tooltip>
{/snippet}

<DropdownMenu bind:open>
	<DropdownMenuTrigger
		class="hover:bg-sidebar-accent hover:text-sidebar-accent-foreground flex w-full min-w-0 items-center gap-2 rounded-xl px-2 py-1.5 text-left outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/50"
	>
		<Avatar>
			<AvatarFallback>{initials}</AvatarFallback>
		</Avatar>
		<span class="flex min-w-0 flex-1 flex-col">
			<span class="truncate text-sm font-medium text-foreground">
				{profile_name ?? "Not connected"}
			</span>
			{#if show_hostname}
				<span class="truncate text-xs text-muted-foreground">{identity?.hostname}</span>
			{/if}
		</span>
	</DropdownMenuTrigger>

	<DropdownMenuContent side="top" align="start" class="w-auto min-w-64 max-w-88">
		<DropdownMenuItem>
			<Settings class="size-4 shrink-0 text-muted-foreground" />
			Settings
		</DropdownMenuItem>

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
			<TooltipProvider delayDuration={300}>
			<div class="flex flex-col gap-2.5 px-1 py-1">
				{#each authenticated_engines as engine (engine.engine_id)}
					{@const mark = EngineMarkFor(engine.engine_id)}
					{@const MarkIcon = mark.icon}
					{@const groups = GroupWindows(engine.windows)}
					<div class="flex flex-col gap-1.5 px-2 py-1">
						<div class="flex items-center gap-2">
							<MarkIcon class={EngineMarkClass(mark, "size-4")} />
							<span class="truncate text-xs font-medium text-foreground">
								{engine.display_name}
							</span>
						</div>
						{#if groups.session.length > 0}
							<div class="flex flex-col gap-1.5">
								{#each groups.session as window (window.id)}
									{@render usage_window(window, mark.accent, is_refreshing)}
								{/each}
							</div>
						{/if}
						{#if groups.extended.length > 0}
							<div class="flex flex-col gap-1.5" class:mt-2={groups.session.length > 0}>
								{#each groups.extended as window (window.id)}
									{@render usage_window(window, mark.accent, is_refreshing)}
								{/each}
							</div>
						{/if}
					</div>
				{/each}

				{#each unavailable_engines as engine (engine.engine_id)}
					{@const mark = EngineMarkFor(engine.engine_id)}
					{@const MarkIcon = mark.icon}
					<div class="flex items-center gap-2 px-2 py-1" title={engine.failure}>
						<MarkIcon class={EngineMarkClass(mark, "size-4")} />
						<span class="truncate text-xs text-muted-foreground">
							{engine.display_name} — usage unavailable
						</span>
					</div>
				{/each}
			</div>
			</TooltipProvider>
		{/if}
	</DropdownMenuContent>
</DropdownMenu>

<style>
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
</style>
