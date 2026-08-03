<script lang="ts" effect>
	import Settings from "@tabler/icons-svelte/icons/settings";
	import { Clock, Effect, Option } from "effect";
	import type { EngineUsageSnapshot, HostIdentitySnapshot } from "@artisan/protocol";
	import { ArtisanClient } from "@artisan/transport/client";
	import { Avatar, AvatarFallback } from "$lib/components/ui/avatar";
	import {
		DropdownMenu,
		DropdownMenuContent,
		DropdownMenuItem,
		DropdownMenuSeparator,
		DropdownMenuTrigger,
	} from "$lib/components/ui/dropdown-menu";
	import ShaderGlassSurface from "./shader-glass-surface.sv";
	import SidebarEngineUsage, { type SidebarUsageState } from "./sidebar-engine-usage.sv";
	import { GradientAvatarSvg } from "$lib/identity/gradient-avatar";
	import { model_manifest } from "@artisan/catalog";
	import {
		EngineUsageCache,
		EngineUsageCacheBrowserLive,
		engine_usage_refresh_is_due,
	} from "$lib/identity/usage-cache";

	const client = yield* ArtisanClient;
	const usage_cache = yield* EngineUsageCache.pipe(Effect.provide(EngineUsageCacheBrowserLive));

	let identity = $state<HostIdentitySnapshot | undefined>(undefined);
	let open = $state(false);
	let usage_state = $state<SidebarUsageState>({ status: "idle" });

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
	/** Engines with a fetch in flight; each header shows its own arc while listed here. */
	let refreshing_engines = $state<ReadonlySet<string>>(new Set());
	const is_refreshing = $derived(refreshing_engines.size > 0);

	/**
	 * Every harness the catalog knows. Engines without a usage surface answer
	 * with an empty report at protocol speed, so over-asking costs nothing and
	 * keeps this list free of a second round trip to discover engine ids.
	 */
	const usage_engine_ids = model_manifest.harnesses.map((harness) => harness.id);

	/** Upserts one engine's reports so each provider paints as soon as it answers. */
	const MergeReports = (incoming: EngineUsageSnapshot) =>
		Effect.gen(function* () {
		const prior = usage_state.status === "loaded" ? usage_state.snapshot.engines : [];
		const by_id = new Map(prior.map((report) => [report.engine_id, report] as const));
		for (const report of incoming.engines) by_id.set(report.engine_id, report);
		usage_state = {
			snapshot: { engines: [...by_id.values()], fetched_at: incoming.fetched_at },
			status: "loaded",
		};
		});

	const FetchEngineUsage = (engine_id: string, force: boolean) =>
		Effect.gen(function* () {
			const snapshot = yield* client.GetEngineUsage({
				engine_id,
				...(force ? { force: true } : {}),
			});
			if (snapshot.engines.length > 0) yield* MergeReports(snapshot);
		}).pipe(
			Effect.catch(() =>
				Effect.gen(function* () {
				}),
			),
			Effect.ensuring(
				Effect.gen(function* () {
					const remaining = new Set(refreshing_engines);
					remaining.delete(engine_id);
					refreshing_engines = remaining;
					if (remaining.size > 0) return;
					/** The fan-out has drained: settle the cache, or the error state when nothing ever loaded. */
					if (usage_state.status === "loaded") {
						yield* usage_cache.Save(usage_state.snapshot).pipe(
							Effect.catch(() =>
								Effect.gen(function* () {
								}),
							),
						);
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
	const RefreshUsage = (force: boolean) =>
		Effect.gen(function* () {
		if (usage_state.status === "loading" || is_refreshing) return;

		refreshing_engines = new Set(usage_engine_ids);
		if (usage_state.status !== "loaded") usage_state = { status: "loading" };
		yield* Effect.forEach(
			usage_engine_ids,
			(engine_id) =>
				Effect.gen(function* () {
					yield* FetchEngineUsage(engine_id, force);
				}),
			{ concurrency: "unbounded", discard: true },
		);
		});

	const RequestUsage = () =>
		Effect.gen(function* () {
		const now_ms = yield* Clock.currentTimeMillis;
		const snapshot = usage_state.status === "loaded" ? usage_state.snapshot : undefined;
		if (!engine_usage_refresh_is_due(snapshot, now_ms)) return;
		yield* RefreshUsage(false);
		});

	/**
	 * The "last checked" reading ages on a coarse scoped clock rather than
	 * freezing at the value from the last menu open.
	 */
	let checked_at_ms = $state(Date.now());
	const TickCheckedAt = Effect.gen(function* () {
		while (true) {
			yield* Effect.sleep("30 seconds");
				checked_at_ms = Date.now();
		}
	});
	yield* TickCheckedAt.pipe(Effect.forkScoped);

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

	const LoadIdentity = Effect.gen(function* () {
		identity = yield* client.GetHostIdentity;
	}).pipe(
		Effect.catch(() =>
			Effect.gen(function* () {
			}),
		),
	);
	const LoadCachedUsage = Effect.gen(function* () {
		const cached_usage = yield* usage_cache.Load;
		if (Option.isSome(cached_usage)) {
			usage_state = { status: "loaded", snapshot: cached_usage.value };
		}
	});
	yield* LoadIdentity;
	yield* LoadCachedUsage;

	// Top-level SER work follows the reactive menu state without a Svelte effect bridge.
	if (open) {
		yield* RequestUsage;
	}
</script>

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

	<DropdownMenuContent
		side="top"
		align="start"
		class="w-auto min-w-64 max-w-88 bg-transparent! p-0! shadow-none! ring-0!"
	>
		<ShaderGlassSurface class="w-full rounded-2xl">
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
			<div class="flex min-w-0 flex-col">
				<span class="truncate text-sm font-medium text-foreground">
					{profile_name ?? "Not connected"}
				</span>
				{#if show_hostname}
					<span class="truncate text-xs text-muted-foreground">{identity?.hostname}</span>
				{/if}
			</div>
		</div>

		<DropdownMenuSeparator />

		<SidebarEngineUsage
			{checked_at_ms}
			{checked_label}
			{is_refreshing}
			onrefresh={RefreshUsage(true)}
			{refreshing_engines}
			{usage_state}
		/>

		<DropdownMenuSeparator />

		<DropdownMenuItem>
			<a href="/settings" class="flex w-full items-center gap-2">
				<Settings class="size-4 shrink-0 text-muted-foreground" />
				Settings
			</a>
		</DropdownMenuItem>
		</ShaderGlassSurface>
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

</style>
