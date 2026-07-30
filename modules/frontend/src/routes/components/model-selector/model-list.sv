<script lang="ts">
	import Star from "@tabler/icons-svelte/icons/star";
	import StarFilled from "@tabler/icons-svelte/icons/star-filled";
	import { EngineMarkClass, ProviderMarkFor } from "$lib/engine/presentation";
	import DropdownHoverSurface from "../dropdown-hover-surface.sv";
	import type { ModelChoice } from "$lib/engine/model-selection";

	let {
		disabled,
		favorite_ids,
		favorites_available,
		models,
		onfavorite,
		onpreview,
		onselect,
		selected_model_id,
	}: {
		disabled: boolean;
		favorite_ids: ReadonlyArray<string>;
		favorites_available: boolean;
		models: ReadonlyArray<ModelChoice>;
		onfavorite: (model_id: string, favorite: boolean) => void;
		onpreview: (model_id: string) => void;
		onselect: (model: ModelChoice) => void;
		selected_model_id: string;
	} = $props();
</script>

<DropdownHoverSurface
	class="pr-2 [--docs-sidebar-hover-radius:calc(var(--radius-3xl)-0.5rem)]"
>
	{#snippet children({ move_hover })}
		<!-- Fixed layout keeps long names inside the scroll box instead of widening the table. -->
		<table
			class="w-full table-fixed border-separate border-spacing-y-0.5"
			aria-label="Available models"
		>
			<tbody>
				{#each models as model (model.id)}
					{@const lab_mark = ProviderMarkFor(model.definition.provider)}
					{@const LabIcon = lab_mark.icon}
					{@const favorited = favorite_ids.includes(model.id)}
					<tr>
						<td class="p-0">
							<div
								role="presentation"
								class="mr-2 flex min-w-0 items-center gap-1"
								onpointerenter={(event) => {
									move_hover(event);
									onpreview(model.id);
								}}
								onpointermove={move_hover}
								onfocusin={move_hover}
							>
								<button
									type="button"
									disabled={disabled || model.definition.disabled !== undefined}
									title={model.definition.disabled?.reason}
									class="flex min-w-0 grow items-center gap-2 rounded-[calc(var(--radius-3xl)-0.5rem)] px-2.5 py-1.5 text-left focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
									aria-current={model.id === selected_model_id ? "true" : undefined}
									onclick={() => onselect(model)}
								>
									<LabIcon class={EngineMarkClass(lab_mark, "size-5")} />
									<span class="flex min-w-0 flex-col space-y-0">
										<span class="truncate text-sm font-semibold text-foreground">
											{model.name}
										</span>
										<span class="truncate text-xs text-muted-foreground">{model.lab}</span>
										{#if model.definition.disabled !== undefined}
											<span class="text-pretty text-xs text-muted-foreground">
												{model.definition.disabled.reason}
											</span>
										{/if}
									</span>
								</button>
								{#if favorites_available}
									<button
										type="button"
										{disabled}
										class="mr-1 grid size-7 shrink-0 place-items-center self-center rounded-full text-muted-foreground transition-colors duration-(--duration-fast) ease-in-out hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45 motion-reduce:transition-none"
										aria-pressed={favorited}
										aria-label={favorited
											? `Unfavorite ${model.name}`
											: `Favorite ${model.name}`}
										onclick={() => onfavorite(model.id, !favorited)}
									>
										{#if favorited}
											<StarFilled class="size-4 text-favorite" aria-hidden="true" />
										{:else}
											<Star class="size-4" aria-hidden="true" />
										{/if}
									</button>
								{/if}
							</div>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/snippet}
</DropdownHoverSurface>
