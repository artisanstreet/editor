<script lang="ts" effect>
	import Refresh from "@tabler/icons-svelte/icons/refresh";
	import { Effect, Stream } from "effect";
	import type { EngineUsageReport } from "@artisan/protocol";
	import { ArtisanClient } from "@artisan/transport/client";
	import { EngineMarkClass, EngineMarkFor, ProviderMarkFor } from "$lib/engine/presentation";
	import { ModelsFromCatalog } from "$lib/engine/model-selection";
	import {
		SessionDefaultsController,
		type SessionDefaultsState,
	} from "$lib/settings/session-defaults-controller";
	import { Badge } from "$lib/components/ui/badge";
	import { Skeleton } from "$lib/components/ui/skeleton";

	let { engine_id }: { engine_id: string } = $props();

	const client = yield* ArtisanClient;
	const defaults_controller = yield* SessionDefaultsController;
	const initial = yield* defaults_controller.Refresh.pipe(
		Effect.catch(() =>
			Effect.gen(function* () {
				return yield* defaults_controller.Current;
			}),
		),
	);
	let defaults_state = $state.raw<SessionDefaultsState>(initial);
	const ApplyDefaults = (next: SessionDefaultsState) =>
		Effect.gen(function* () {
			defaults_state = next;
		});
	yield* defaults_controller.Changes.pipe(
		Stream.runForEach(ApplyDefaults),
		Effect.forkScoped,
	);
	const runtime_catalog = $derived(defaults_state.catalog);
	const forge_available = $derived(defaults_state.available);
	const catalog_models = $derived(ModelsFromCatalog(runtime_catalog));

	const harness = $derived(
		runtime_catalog.manifest.harnesses.find((candidate) => candidate.id === engine_id),
	);
	const engine_mark = $derived(EngineMarkFor(engine_id));
	const engine_models = $derived(catalog_models.filter((model) => model.engine === engine_id));
	const compaction_default = $derived(
		catalog_models.find((model) => model.id === harness?.compaction_default_model_id),
	);

	let usage = $state<EngineUsageReport | undefined>(undefined);
	let usage_loaded = $state(false);
	let usage_refreshing = $state(false);

	const StoreUsage = (report: EngineUsageReport | undefined) =>
		Effect.gen(function* () {
			usage = report;
			usage_loaded = true;
			usage_refreshing = false;
		});

	const LoadUsage = (force: boolean) =>
		Effect.gen(function* () {
			const snapshot = yield* client.GetEngineUsage({ engine_id, ...(force ? { force: true } : {}) });
			yield* StoreUsage(snapshot.engines.find((report) => report.engine_id === engine_id));
		}).pipe(
			Effect.catch(() =>
				Effect.gen(function* () {
					yield* StoreUsage(undefined);
				}),
			),
		);

	const RefreshUsage = Effect.gen(function* () {
		if (!forge_available || usage_refreshing) return;
		usage_refreshing = true;
		yield* LoadUsage(true);
	});

	/** `engine_id` is a reactive input: navigating between engine pages refetches. */
	yield* LoadUsage(false);

	/** Only paint a report that belongs to the page being viewed. */
	const current_usage = $derived(usage?.engine_id === engine_id ? usage : undefined);
	const window_kind_labels: Readonly<Record<string, string>> = {
		monthly: "Monthly",
		session: "Session",
		unknown: "Usage",
		weekly: "Weekly",
	};
	const window_reset_label = (resets_at: string | undefined) =>
		resets_at === undefined
			? undefined
			: `Resets ${new Date(resets_at).toLocaleString(undefined, {
					day: "numeric",
					hour: "numeric",
					minute: "2-digit",
					month: "short",
				})}`;
</script>

{#if harness === undefined}
	<h1 class="text-lg font-semibold text-foreground">Unknown engine</h1>
	<p class="mt-1 text-sm text-muted-foreground">
		No engine with id "{engine_id}" exists in the catalog.
	</p>
{:else}
	{@const EngineIcon = engine_mark.icon}
	<h1 class="flex items-center gap-2.5 text-lg font-semibold text-foreground">
		<EngineIcon
			class={engine_mark.monochrome ? "size-5 shrink-0 dark:invert" : "size-5 shrink-0"}
		/>
		{harness.label}
	</h1>
	<p class="mt-1 text-sm text-muted-foreground">
		Account, catalog, and permission surface for the {harness.label} engine.
	</p>

	<section class="mt-10" aria-labelledby="account">
		<div class="flex items-center justify-between gap-4">
			<h2 id="account" class="scroll-mt-6 text-sm font-medium text-foreground">Account</h2>
			<button
				type="button"
				disabled={!forge_available || usage_refreshing}
				class="flex items-center gap-1.5 rounded-md px-2 py-1 text-xs text-muted-foreground hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
				onclick={yield* RefreshUsage}
			>
				<Refresh
					class={usage_refreshing
						? "size-3.5 shrink-0 animate-spin motion-reduce:animate-none"
						: "size-3.5 shrink-0"}
					aria-hidden="true"
				/>
				{usage_refreshing ? "Refreshing…" : "Refresh"}
			</button>
		</div>
		<div
			class="card mt-3 rounded-xl bg-linear-to-b from-surface-225 to-surface-200 dark:from-surface-800 dark:to-surface-925"
		>
			{#if !usage_loaded}
				<div class="flex flex-col gap-5 px-5 py-5">
					<Skeleton class="h-4 w-44" />
					<Skeleton class="h-3 w-32" />
					<Skeleton class="h-1.5 w-full" />
					<Skeleton class="h-3 w-24" />
					<Skeleton class="h-1.5 w-full" />
				</div>
			{:else}
				<div class="settings-usage-enter flex flex-col gap-5 px-5 py-5">
					{#if current_usage === undefined}
						<span class="text-sm text-muted-foreground">Account unavailable</span>
					{:else if current_usage.authentication === "authenticated"}
						<span class="text-sm text-foreground">
							{#if current_usage.account_email}
								Signed in as
								<span
									class="blur-[5px] transition-[filter] duration-(--duration-fast) ease-in-out hover:blur-none motion-reduce:transition-none"
								>
									{current_usage.account_email}
								</span>
							{:else}
								Signed in
							{/if}
						</span>
					{:else if current_usage.authentication === "unauthenticated"}
						<span class="text-sm text-destructive">Not signed in</span>
					{:else}
						<span class="text-sm text-muted-foreground">Sign-in status unknown</span>
					{/if}
					{#if current_usage?.failure !== undefined}
						<p class="text-pretty text-xs text-muted-foreground">
							{current_usage.failure}
						</p>
					{/if}
					{#if current_usage !== undefined && current_usage.windows.length > 0}
						<div class="flex flex-col gap-4">
							{#each current_usage.windows as usage_window (usage_window.id)}
								<div class="flex flex-col gap-1">
									<div class="flex items-center justify-between gap-4 text-xs">
										<span class="truncate text-muted-foreground">
											{usage_window.label ??
												window_kind_labels[usage_window.kind] ??
												"Usage"}
										</span>
										<span class="shrink-0 text-foreground">
											{Math.round(usage_window.percent_used)}%
										</span>
									</div>
									<div
										class="h-1.5 w-full overflow-hidden rounded-full bg-surface-500/30"
									>
										<div
											class="h-full rounded-full bg-foreground/70 transition-[width] duration-(--duration-quick) ease-(--ease-smooth-out) motion-reduce:transition-none"
											style={`width: ${Math.min(usage_window.percent_used, 100)}%`}
										></div>
									</div>
									{#if window_reset_label(usage_window.resets_at) !== undefined}
										<span class="text-[0.7rem] text-muted-foreground">
											{window_reset_label(usage_window.resets_at)}
										</span>
									{/if}
								</div>
							{/each}
						</div>
					{:else if current_usage !== undefined && current_usage.authentication === "authenticated"}
						<p class="text-xs text-muted-foreground">
							This provider does not expose a quota surface.
						</p>
					{/if}
				</div>
			{/if}
		</div>
	</section>

	<section class="mt-10" aria-labelledby="models">
		<h2 id="models" class="scroll-mt-6 text-sm font-medium text-foreground">Models</h2>
		<div
			class="card mt-3 rounded-xl bg-linear-to-b from-surface-225 to-surface-200 dark:from-surface-800 dark:to-surface-925"
		>
			<div class="flex flex-col divide-y divide-border/40">
				{#each engine_models as model (model.id)}
					{@const lab_mark = ProviderMarkFor(model.definition.provider)}
					{@const LabIcon = lab_mark.icon}
					<div class="flex items-center justify-between gap-4 px-4 py-2.5">
						<span class="flex min-w-0 items-center gap-2.5">
							<LabIcon class={EngineMarkClass(lab_mark, "size-4 shrink-0")} />
							<span class="flex min-w-0 flex-col">
								<span class="truncate text-sm text-foreground">{model.name}</span>
								{#if model.definition.description !== undefined}
									<span class="truncate text-xs text-muted-foreground">
										{model.definition.description}
									</span>
								{/if}
							</span>
						</span>
						<span class="flex shrink-0 items-center gap-1.5">
							{#if model.id === compaction_default?.id}
								<Badge variant="outline">Compaction default</Badge>
							{/if}
							{#if model.definition.disabled !== undefined}
								<Badge variant="outline">Disabled</Badge>
							{/if}
						</span>
					</div>
				{/each}
			</div>
		</div>
	</section>

	<section class="mt-10" aria-labelledby="permissions">
		<h2 id="permissions" class="scroll-mt-6 text-sm font-medium text-foreground">
			Permissions
		</h2>
		<div
			class="card mt-3 rounded-xl bg-linear-to-b from-surface-225 to-surface-200 dark:from-surface-800 dark:to-surface-925"
		>
			<div class="flex flex-col divide-y divide-border/40">
				{#each harness.permissions.options as option (option.id)}
					<div class="flex items-center justify-between gap-4 px-4 py-2.5">
						<span class="flex min-w-0 flex-col">
							<span class="truncate text-sm text-foreground">{option.label}</span>
							<span class="text-pretty text-xs text-muted-foreground">
								{option.description}
							</span>
						</span>
						{#if option.id === harness.permissions.default}
							<Badge variant="outline">Default</Badge>
						{/if}
					</div>
				{/each}
			</div>
		</div>
	</section>
{/if}

<style>
	/*
	 * The card's content replaces skeletons in place; a short rise-and-settle
	 * keeps the swap from reading as a pop. Pure CSS: transition directives
	 * deadlock the async renderer.
	 */
	.settings-usage-enter {
		animation: settings-usage-enter var(--duration-quick) var(--ease-smooth-out);
	}

	@keyframes settings-usage-enter {
		from {
			opacity: 0;
			transform: translateY(0.25rem);
		}

		to {
			opacity: 1;
			transform: translateY(0);
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.settings-usage-enter {
			animation: none;
		}
	}
</style>
