<script lang="ts" effect>
	import Settings from "@tabler/icons-svelte/icons/settings";
	import { Effect, Queue } from "effect";
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
	import { EngineMarkClass, EngineMarkFor } from "$lib/engine/presentation";

	type UsageState =
		| { readonly status: "idle" }
		| { readonly status: "loading" }
		| { readonly status: "loaded"; readonly snapshot: EngineUsageSnapshot }
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

	const WindowLabel = (window: EngineUsageWindow): string =>
		window.label ?? window_kind_labels[window.kind];

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

	let identity = $state<HostIdentitySnapshot | undefined>(undefined);
	let open = $state(false);
	let usage_state = $state<UsageState>({ status: "idle" });

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
			Effect.sync(() => {
				usage_state = { status: "loaded", snapshot };
			}),
		),
		Effect.catch(() =>
			Effect.sync(() => {
				usage_state = { status: "error" };
			}),
		),
	);

	const RequestUsage = () => {
		if (usage_state.status !== "idle") return;
		usage_state = { status: "loading" };
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

	yield* Queue.take(usage_requests).pipe(
		Effect.flatMap(() => FetchUsage),
		Effect.forever,
		Effect.forkScoped,
	);

	$effect(() => {
		if (open) RequestUsage();
	});
</script>

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

	<DropdownMenuContent side="top" align="start" class="w-64">
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
			<div class="flex flex-col gap-2.5 px-1 py-1">
				{#each authenticated_engines as engine (engine.engine_id)}
					{@const mark = EngineMarkFor(engine.engine_id)}
					{@const MarkIcon = mark.icon}
					<div class="flex flex-col gap-1.5 px-2 py-1">
						<div class="flex items-center gap-2">
							<MarkIcon class={EngineMarkClass(mark, "size-4")} />
							<span class="truncate text-xs font-medium text-foreground">
								{engine.display_name}
							</span>
						</div>
						<div class="flex flex-col gap-1.5">
							{#each engine.windows as window (window.id)}
								<div class="flex flex-col gap-0.5">
									<div class="flex items-center justify-between gap-2 text-xs text-muted-foreground">
										<span class="truncate">{WindowLabel(window)}</span>
										<span class="shrink-0 tabular-nums">
											{Math.round(window.percent_used)}%{window.resets_at
												? ` · ${RelativeReset(window.resets_at)}`
												: ""}
										</span>
									</div>
									<div class="bg-muted h-1 w-full overflow-hidden rounded-full">
										<div
											class="h-full rounded-full bg-primary"
											style={`width: ${Math.min(100, Math.max(0, window.percent_used))}%`}
										></div>
									</div>
								</div>
							{/each}
						</div>
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
		{/if}
	</DropdownMenuContent>
</DropdownMenu>
