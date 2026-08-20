<script lang="ts" effect>
	import Settings from "@tabler/icons-svelte/icons/settings";
	import { Clock, Effect, Option, Stream } from "effect";
	import type { EngineUsageSnapshot, HostIdentitySnapshot } from "@artisan/protocol";
	import { Avatar, AvatarFallback } from "$lib/components/ui/avatar";
	import {
		DropdownMenu,
		DropdownMenuContent,
		DropdownMenuItem,
		DropdownMenuSeparator,
		DropdownMenuTrigger,
	} from "$lib/components/ui/dropdown-menu";
	import { MakeFollowHighlight } from "$lib/components/dropdown-highlight";
	import DropdownHoverSurface from "./dropdown-hover-surface.svelte";
	import ShaderGlassSurface from "./shader-glass-surface.svelte";
	import SidebarEngineUsage, { type SidebarUsageState } from "./sidebar-engine-usage.svelte";
	import { GradientAvatarSvg } from "$lib/identity/gradient-avatar";
	import {
		SessionDefaultsController,
		type SessionDefaultsState,
	} from "$lib/settings/session-defaults-controller";
	import { model_manifest } from "@artisan/catalog";
	import {
		EngineUsageCache,
		EngineUsageCacheBrowserLive,
		engine_usage_refresh_is_due,
	} from "$lib/identity/usage-cache";
	import {
		EngineUsageController,
		type EngineUsageEntry,
		type EngineUsageState,
	} from "$lib/identity/engine-usage-controller";
	import { HostIdentityController } from "$lib/identity/host-identity-controller";

	const usage_cache = yield* EngineUsageCache.pipe(Effect.provide(EngineUsageCacheBrowserLive));
	const FollowHighlight = yield* MakeFollowHighlight;

	let {
		open = $bindable(false),
	}: {
		/**
		 * Mirrored out because the menu paints over the transcript's left margin:
		 * surfaces that read proximity there need to know they are covered.
		 */
		open?: boolean;
	} = $props();

	const identity_controller = yield* HostIdentityController;
	let identity = $state<HostIdentitySnapshot | undefined>(yield* identity_controller.Current);
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
	const usage_controller = yield* EngineUsageController;
	/** Engines with a fetch in flight; each header shows its own arc while listed here. */
	let refreshing_engines = $state.raw<ReadonlySet<string>>(
		(yield* usage_controller.Current).refreshing_engine_ids,
	);
	/**
	 * Hoisted so the yield site never names the state it writes. An inline arrow
	 * puts `refreshing_engines` in the site's reactive inputs, and because these
	 * `Changes` streams replay their current value on subscribe, the write would
	 * re-invalidate the site, resubscribe, and replay again — an unbounded loop
	 * that leaks a subscription and a scoped fiber per turn.
	 */
	/** Every engine's own reading, handed to the menu unaggregated. */
	let usage_entries = $state.raw<ReadonlyMap<string, EngineUsageEntry>>(
		(yield* usage_controller.Current).entries,
	);
	const ApplyUsageState = (next: EngineUsageState) =>
		Effect.gen(function* () {
			refreshing_engines = next.refreshing_engine_ids;
			usage_entries = next.entries;
			const entries = [...next.entries.values()];
			if (entries.length === 0) return;
			const reports = entries
				.map((entry) => entry.report)
				.filter((report): report is NonNullable<typeof report> => report !== undefined);
			/**
			 * The aggregate survives only as the cache's own shape, which is one
			 * document with one stamp. Nothing on screen reads it: each row takes
			 * its reading and its timestamp from its own entry above.
			 */
			usage_state = {
				status: "loaded",
				snapshot: {
					engines: reports,
					fetched_at: new Date(
						Math.max(...entries.map((entry) => entry.fetched_at_ms)),
					).toISOString(),
				},
			};
		});
	yield* usage_controller.Changes.pipe(
		Stream.runForEach(ApplyUsageState),
		Effect.forkScoped,
	);
	const ApplyIdentity = (next: HostIdentitySnapshot | undefined) =>
		Effect.gen(function* () {
			identity = next;
		});
	yield* identity_controller.Changes.pipe(Stream.runForEach(ApplyIdentity), Effect.forkScoped);

	const defaults_controller = yield* SessionDefaultsController;
	let disabled_engine_ids = $state.raw<ReadonlyArray<string>>(
		(yield* defaults_controller.Current).defaults.disabled_engines ?? [],
	);
	const ApplyDisabledEngineIds = (next: SessionDefaultsState) =>
		Effect.gen(function* () {
			disabled_engine_ids = next.defaults.disabled_engines ?? [];
		});
	yield* defaults_controller.Changes.pipe(
		Stream.runForEach(ApplyDisabledEngineIds),
		Effect.forkScoped,
	);

	/**
	 * Every harness the catalog knows, minus the ones the user switched off.
	 * A disabled engine is not represented anywhere — asking its provider for
	 * usage would resurrect it as a pending skeleton in this very menu.
	 * Engines without a usage surface answer with an empty report at protocol
	 * speed, so over-asking the enabled set costs nothing.
	 */
	const usage_engine_ids = $derived(
		model_manifest.harnesses
			.map((harness) => harness.id)
			.filter((engine_id) => !disabled_engine_ids.includes(engine_id)),
	);

	/**
	 * Persists whatever has answered so far.
	 *
	 * Run per engine rather than once after the slowest, so a provider that never
	 * returns cannot keep the engines that did out of the cache — and no longer
	 * flips a shared status the rows would all have obeyed.
	 */
	const SettleUsage = Effect.gen(function* () {
		if (usage_state.status !== "loaded") return;
		yield* usage_cache.Save(usage_state.snapshot).pipe(
			Effect.catch(() =>
				Effect.gen(function* () {
				}),
			),
		);
	});

	/**
	 * Fans one fetch out per engine; the manual control, the mount prefetch,
	 * and the menu-open freshness check share it. Manual refreshes force past
	 * the backend's freshness window, so
	 * the providers are genuinely re-asked; the automatic session fetch accepts
	 * backend-cached reports. Results merge in as each provider answers rather
	 * than waiting for the slowest one.
	 */
	const RefreshUsage = (force: boolean, requested_engine_ids = usage_engine_ids) =>
		Effect.gen(function* () {
		if (usage_state.status !== "loaded") usage_state = { status: "loading" };
		/**
		 * Each engine settles the moment it answers. Waiting for the whole
		 * fan-out to persist meant one slow provider withheld every other
		 * engine's reading from the cache the next session opens on.
		 */
		yield* Effect.forEach(
			requested_engine_ids,
			(engine_id) =>
				Effect.gen(function* () {
					yield* usage_controller.Load(engine_id, { force }).pipe(Effect.ignore);
					yield* SettleUsage;
				}),
			{ concurrency: "unbounded", discard: true },
		);
		});
	/** A row refresh is deliberately scoped to its provider; background freshness still fans out. */
	const RefreshEngineUsage = (engine_id: string) => RefreshUsage(true, [engine_id]);

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

	const LoadCachedUsage = Effect.gen(function* () {
		const cached_usage = yield* usage_cache.Load;
		if (Option.isSome(cached_usage)) {
			usage_state = { status: "loaded", snapshot: cached_usage.value };
			yield* usage_controller.Seed(cached_usage.value);
		}
	});
	yield* identity_controller.Refresh.pipe(Effect.forkScoped);
	/**
	 * The fan-out starts at mount, not at first open: the enabled set is
	 * already known from the catalog, so by the time the menu opens the
	 * readings are usually fetched and paint immediately instead of the
	 * whole menu waiting on the slowest provider's first answer.
	 */
	yield* Effect.gen(function* () {
		yield* LoadCachedUsage;
		yield* RequestUsage();
	}).pipe(Effect.forkScoped);

	// Top-level SER work follows the reactive menu state without a Svelte effect bridge.
	if (open) {
		yield* RequestUsage().pipe(Effect.forkScoped);
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
			entries={usage_entries}
			engine_ids={usage_engine_ids}
			hidden_engine_ids={disabled_engine_ids}
			onrefresh={RefreshEngineUsage}
			{refreshing_engines}
			{usage_state}
		/>

		<DropdownMenuSeparator />

		<!--
			Inset like every other section of this menu, and lit by the same
			travelling pill the app's other dropdowns use rather than the item's own
			block highlight.
		-->
		<div class="p-1">
			<DropdownHoverSurface class="[--docs-sidebar-hover-radius:var(--radius-xl)]">
				{#snippet children({ move_hover })}
					<DropdownMenuItem
						class="rounded-xl focus:bg-transparent! data-highlighted:bg-transparent! data-highlighted:text-foreground!"
						{@attach FollowHighlight(move_hover)}
					>
						<a href="/settings" class="flex w-full items-center gap-2">
							<Settings class="size-4 shrink-0 text-muted-foreground" />
							Settings
						</a>
					</DropdownMenuItem>
				{/snippet}
			</DropdownHoverSurface>
		</div>
		</ShaderGlassSurface>
	</DropdownMenuContent>
</DropdownMenu>
