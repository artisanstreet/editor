<script lang="ts" effect>
	import StarFilled from "@tabler/icons-svelte/icons/star-filled";
	import X from "@tabler/icons-svelte/icons/x";
	import { Effect, Stream } from "effect";
	import { BannerService } from "$lib/banner/service";
	import { EngineMarkClass, ProviderMarkFor } from "$lib/engine/presentation";
	import { ModelsFromCatalog } from "$lib/engine/model-selection";
	import {
		SessionDefaultsController,
		type SessionDefaultsState,
	} from "$lib/settings/session-defaults-controller";
	import CompactionModel from "./compaction-model.sv";

	const banner = yield* BannerService;
	const defaults_controller = yield* SessionDefaultsController;
	const initial = yield* defaults_controller.Refresh.pipe(
		Effect.catch((error) =>
			Effect.gen(function* () {
				yield* banner.error("Could not load model defaults", { description: error.message });
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
	const models = $derived(ModelsFromCatalog(runtime_catalog));

	/** The Forge-owned starred set, shared with every picker. */
	const favorites = $derived(
		defaults_state.favorite_ids
			.map((id) => models.find((model) => model.id === id))
			.filter((model): model is NonNullable<typeof model> => model !== undefined),
	);

	const Unstar = (model_id: string) =>
		Effect.gen(function* () {
			if (!forge_available) return;
			const next = yield* defaults_controller.SetFavorite(model_id, false);
			yield* ApplyDefaults(next);
		}).pipe(
			Effect.catch((error) =>
				Effect.gen(function* () {
					yield* banner.error("Could not update model favorites", {
						description: error.message,
					});
					const current = yield* defaults_controller.Current;
					yield* ApplyDefaults(current);
				}),
			),
		);
</script>

<h1 class="text-lg font-semibold text-foreground">Models</h1>
<p class="mt-1 text-sm text-muted-foreground">
	Forge-owned model defaults shared by every paired client.
</p>

<section class="mt-10" aria-labelledby="compaction">
	<h2 id="compaction" class="scroll-mt-6 text-sm font-medium text-foreground">Compaction</h2>
	<div class="mt-3">
		<CompactionModel />
	</div>
</section>

<section class="mt-10" aria-labelledby="favorites">
	<h2 id="favorites" class="scroll-mt-6 text-sm font-medium text-foreground">Favorites</h2>
	<div
		class="card mt-3 rounded-xl bg-linear-to-b from-surface-225 to-surface-200 dark:from-surface-800 dark:to-surface-925"
	>
		{#if favorites.length === 0}
			<p class="px-4 py-3.5 text-xs text-muted-foreground">
				No favorites yet. Starred models float to the top of every model picker; star
				one from the composer's picker or an engine page.
			</p>
		{:else}
			<div class="flex flex-col divide-y divide-border/40">
				{#each favorites as model (model.id)}
					{@const lab_mark = ProviderMarkFor(model.definition.provider)}
					{@const LabIcon = lab_mark.icon}
					<div class="flex items-center justify-between gap-4 px-4 py-2.5">
						<span class="flex min-w-0 items-center gap-2.5">
							<LabIcon class={EngineMarkClass(lab_mark, "size-4 shrink-0")} />
							<span class="flex min-w-0 flex-col">
								<span class="flex items-center gap-1.5 truncate text-sm text-foreground">
									<StarFilled class="size-3 shrink-0 text-favorite" aria-hidden="true" />
									{model.name}
								</span>
								<span class="truncate text-xs text-muted-foreground">{model.lab}</span>
							</span>
						</span>
						<button
							type="button"
							disabled={!forge_available}
							aria-label={`Unfavorite ${model.name}`}
							class="grid size-7 shrink-0 place-items-center rounded-full text-muted-foreground hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
							onclick={yield* Unstar(model.id)}
						>
							<X class="size-4" aria-hidden="true" />
						</button>
					</div>
				{/each}
			</div>
		{/if}
	</div>
</section>
