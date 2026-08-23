<script lang="ts" effect>
	import StarFilled from "@tabler/icons-svelte/icons/star-filled";
	import X from "@tabler/icons-svelte/icons/x";
	import { Effect, Stream } from "effect";
	import { EngineMarkClass, ProviderMarkFor } from "$lib/engine/presentation";
	import { ModelsFromCatalog, VariantLabel } from "$lib/engine/model-selection";
	import { Button } from "$lib/components/ui/button";
	import {
		SessionDefaultsController,
		type SessionDefaultsState,
	} from "$lib/settings/session-defaults-controller";
	import Card from "./card.svelte";
	import CompactionModel from "./compaction-model.svelte";
	import Header from "./header.svelte";
	import Section from "./section.svelte";

	const defaults_controller = yield* SessionDefaultsController;
	const initial = yield* defaults_controller.Current;
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
			Effect.catch(() =>
				Effect.gen(function* () {
					const current = yield* defaults_controller.Current;
					yield* ApplyDefaults(current);
				}),
			),
		);
</script>

<Header title="Models" description="Forge-owned model defaults shared by every paired client." />

<Section id="compaction" title="Compaction">
	<div class="mt-3">
		<CompactionModel />
	</div>
</Section>

<Section id="favorites" title="Favorites">
	<Card class="mt-3">
		{#if favorites.length === 0}
			<p class="max-w-sm self-center px-4 py-7 text-center text-xs leading-relaxed text-muted-foreground">
				No favorites yet. Starred models float to the top of every model picker; star
				one from the composer's picker or an engine page.
			</p>
		{:else}
			{#each favorites as model (model.id)}
				{@const lab_mark = ProviderMarkFor(model.definition.provider)}
				{@const LabIcon = lab_mark.icon}
				{@const variant_id = model.definition.native_selection?.variant_id}
				<div class="flex items-center justify-between gap-4 px-4 py-2.5">
					<span class="flex min-w-0 items-center gap-2.5">
						<LabIcon class={EngineMarkClass(lab_mark, "size-4 shrink-0")} />
						<span class="flex min-w-0 flex-col">
							<span class="flex items-center gap-1.5 truncate text-sm text-foreground">
							<StarFilled class="size-3 shrink-0 text-favorite" aria-hidden="true" />
							{model.name}
							{#if variant_id !== undefined}
								<span class="text-muted-foreground">{VariantLabel(model)}</span>
							{/if}
							</span>
							<span class="truncate text-xs text-muted-foreground">{model.lab}</span>
						</span>
					</span>
					<Button
						variant="ghost"
						size="icon-sm"
						disabled={!forge_available}
						aria-label={`Unfavorite ${model.name}`}
						class="rounded-full text-muted-foreground"
						onclick={yield* Unstar(model.id)}
					>
						<X class="size-4" aria-hidden="true" />
					</Button>
				</div>
			{/each}
		{/if}
	</Card>
</Section>
