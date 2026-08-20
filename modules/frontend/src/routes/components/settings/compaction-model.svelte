<script lang="ts" effect>
	import ArrowsMinimize from "@tabler/icons-svelte/icons/arrows-minimize";
	import Selector from "@tabler/icons-svelte/icons/selector";
	import Tool from "@tabler/icons-svelte/icons/tool";
	import type { Component } from "svelte";
	import { Effect, Stream } from "effect";
	import type { SessionDefaults } from "@artisan/protocol";
	import { MakeFollowHighlight } from "$lib/components/dropdown-highlight";
	import artisan_star from "$lib/assets/barekey/artisan-star.svg";
	import { EngineMarkClass, EngineMarkFor } from "$lib/engine/presentation";
	import {
		CompactionSelectionFromDefaults,
		SessionDefaultsController,
		type CompactionSelection,
		type SessionDefaultsState,
	} from "$lib/settings/session-defaults-controller";
	import {
		ModelsFromCatalog,
		OrderModels,
		thinking_level_labels,
		type EngineChoice,
		type HarnessId,
		type ModelChoice,
		type PermissionOption,
	} from "$lib/engine/model-selection";
	import { Popover, PopoverContent, PopoverTrigger } from "$lib/components/ui/popover";
	import { Select, SelectContent, SelectItem, SelectTrigger } from "$lib/components/ui/select";
	import { Tabs } from "$lib/components/ui/tabs";
	import DropdownHoverSurface from "../dropdown-hover-surface.svelte";
	import EngineSection from "../model-selector/engine-section.svelte";
	import ModelList from "../model-selector/model-list.svelte";
	import {
		ContextForDefaults,
		PermissionsForSelection,
		ThinkingForDefaults,
	} from "../model-selector/presentation";
	import ShaderGlassSurface from "../shader-glass-surface.svelte";

	const FollowHighlight = yield* MakeFollowHighlight;
	const defaults_controller = yield* SessionDefaultsController;
	const initial = yield* defaults_controller.Current;
	let defaults_state = $state.raw<SessionDefaultsState>(initial);
	const runtime_catalog = $derived(defaults_state.catalog);
	const model_manifest = $derived(runtime_catalog.manifest);
	const forge_available = $derived(defaults_state.available);
	const models = $derived(ModelsFromCatalog(runtime_catalog));
	const engines: ReadonlyArray<EngineChoice> = $derived(
		model_manifest.harnesses.map((harness) => ({
			id: harness.id,
			name: harness.label,
			...EngineMarkFor(harness.id),
		})),
	);
	const curated_rows = $derived(
		model_manifest.harnesses
			.map((harness) => {
				const model = models.find(
					(candidate) => candidate.id === harness.compaction_default_model_id,
				);
				const engine = engines.find((candidate) => candidate.id === harness.id);
				return model === undefined || engine === undefined
					? undefined
					: { engine, model_name: model.name };
			})
			.filter((row): row is NonNullable<typeof row> => row !== undefined),
	);
	const barekey_mark_style = [
		"background-color: var(--foreground)",
		`mask-image: url(${artisan_star})`,
		"mask-size: contain",
		"mask-repeat: no-repeat",
		"mask-position: center",
		`-webkit-mask-image: url(${artisan_star})`,
		"-webkit-mask-size: contain",
		"-webkit-mask-repeat: no-repeat",
		"-webkit-mask-position: center",
	].join("; ");

	const defaults = $derived(defaults_state.defaults);
	const compaction_selection = $derived(CompactionSelectionFromDefaults(defaults));
	let picker_open = $state(false);
	let active_engine = $state<HarnessId>(
		ModelsFromCatalog(initial.catalog)[0]?.engine ?? "codex",
	);
	let previewed_model_id = $state<string | undefined>(undefined);
	let previewed_mode = $state<"curated" | "inherited" | undefined>(undefined);
	const inherited = $derived(compaction_selection._tag === "Inherited");
	const compaction_model = $derived(
		compaction_selection._tag === "Explicit"
			? models.find((model) => model.id === compaction_selection.model_id)
			: undefined,
	);
	const active_engine_choice = $derived(
		engines.find((engine) => engine.id === active_engine) ??
			engines[0] ?? { icon: Tool, id: "codex", monochrome: true, name: "Unavailable" },
	);
	/**
	 * Forge owns the starred set and both pickers read it from the same snapshot,
	 * so a model starred in the composer opens starred here and in the same
	 * order.
	 */
	const favorite_ids = $derived(defaults_state.favorite_ids);
	const active_models = $derived(OrderModels(models, active_engine, favorite_ids));

	const RequestFavorite = (model_id: string, favorite: boolean) =>
		Effect.gen(function* () {
			if (!forge_available) return;
			yield* defaults_controller.SetFavorite(model_id, favorite).pipe(
				Effect.catch(() =>
					Effect.gen(function* () {
					}),
				),
			);
		});

	const PreviewModelId = (model_id: string) =>
		Effect.gen(function* () {
			previewed_model_id = model_id;
			previewed_mode = undefined;
		});
	const previewed_model = $derived(
		models.find((model) => model.id === previewed_model_id) ?? compaction_model,
	);
	const previewed_permissions = $derived(
		previewed_model === undefined
			? []
			: PermissionsForSelection(
					previewed_model,
					model_manifest.harnesses.find((harness) => harness.id === previewed_model.engine)
						?.permissions.options ?? [],
				),
	);
	const previewed_pane = $derived<"curated" | "inherited" | "model">(
		previewed_mode !== undefined
			? previewed_mode
			: previewed_model_id !== undefined
				? "model"
				: inherited
					? "inherited"
					: compaction_model === undefined
						? "curated"
						: "model",
	);
	const TriggerIcon = $derived<Component | undefined>(
		compaction_model === undefined
			? undefined
			: engines.find((engine) => engine.id === compaction_model.engine)?.icon,
	);
	const trigger_monochrome = $derived(
		compaction_model !== undefined &&
			(engines.find((engine) => engine.id === compaction_model.engine)?.monochrome ?? false),
	);
	const trigger_label = $derived(inherited ? "Inherited" : (compaction_model?.name ?? "Curated"));


	const ApplyDefaults = (next: SessionDefaultsState) =>
		Effect.gen(function* () {
			defaults_state = next;
			const selection = CompactionSelectionFromDefaults(next.defaults);
			const next_models = ModelsFromCatalog(next.catalog);
			const configured =
				selection._tag === "Explicit"
					? next_models.find((model) => model.id === selection.model_id)
					: undefined;
			const next_engine =
				configured?.engine ??
				next_models.find((model) => model.engine === active_engine)?.engine ??
				next_models[0]?.engine;
			if (next_engine !== undefined) active_engine = next_engine;
		});

	const SaveDefaults = (
		selection: CompactionSelection,
		options: {
			readonly model?: SessionDefaults["models"][number];
			readonly permission?: string;
		} = {},
	) =>
		Effect.gen(function* () {
			const next = yield* defaults_controller.SaveCompactionDefaults({
				...options,
				selection,
			});
			yield* ApplyDefaults(next);
		}).pipe(
			Effect.catch(() =>
				Effect.gen(function* () {
					yield* ApplyDefaults(yield* defaults_controller.Current);
				}),
			),
		);

	const SelectSelection = (selection: CompactionSelection) =>
		Effect.gen(function* () {
			yield* SaveDefaults(selection);
			picker_open = false;
			previewed_model_id = undefined;
			previewed_mode = undefined;
		});

	const SelectModel = (model: ModelChoice) =>
		Effect.gen(function* () {
			yield* SelectSelection({ _tag: "Explicit", model_id: model.id });
		});

	const PreviewModel = (model: ModelChoice) =>
		Effect.gen(function* () {
			previewed_model_id = model.id;
			previewed_mode = undefined;
		});

	const PreviewMode = (mode: "curated" | "inherited") =>
		Effect.gen(function* () {
			previewed_mode = mode;
			previewed_model_id = undefined;
		});

	/**
	 * The chosen value arrives as an argument, never off the event: a SER handler
	 * runs after its event has finished dispatching, by which time
	 * `event.currentTarget` is null and every one of these reads returned early.
	 */
	const UpdateThinking = (value: string) =>
		Effect.gen(function* () {
			if (previewed_model === undefined) return;
			const capability = previewed_model.definition.capabilities.thinking;
			if (capability.availability !== "supported") return;
			const level = capability.options.find((option) => option.id === value)?.id;
			if (level === undefined) return;
			yield* SaveDefaults(
				{ _tag: "Explicit", model_id: previewed_model.id },
				{
					model: {
						model_id: previewed_model.id,
						reasoning_effort: level === "light" ? "low" : level,
					},
				},
			);
		});

	const UpdateContext = (value: string) =>
		Effect.gen(function* () {
			if (previewed_model === undefined) return;
			const context = previewed_model.definition.capabilities.context_window;
			const option = context?.options.find((candidate) => candidate.id === value);
			if (option === undefined) return;
			yield* SaveDefaults(
				{ _tag: "Explicit", model_id: previewed_model.id },
				{
					model: {
						...(option.native_suffix === "" ? {} : { context_window: option.native_suffix }),
						model_id: previewed_model.id,
					},
				},
			);
		});

	const UpdatePermission = (value: string) =>
		Effect.gen(function* () {
			if (previewed_model === undefined) return;
			const option = previewed_permissions.find((candidate) => candidate.id === value);
			if (option === undefined) return;
			yield* SaveDefaults(
				{ _tag: "Explicit", model_id: previewed_model.id },
				{ permission: option.id },
			);
		});

	yield* defaults_controller.Changes.pipe(
		Stream.runForEach(ApplyDefaults),
		Effect.forkScoped,
	);

</script>

{#snippet barekey_mark(size_class)}
	<span aria-hidden="true" class="{size_class} shrink-0" style={barekey_mark_style}></span>
{/snippet}

{#snippet mode_row(input, move_hover)}
	<!--
		The pill handlers are direct references and the preview is a separate
		effectful handler: an effectful handler runs after its event finished
		dispatching, when `currentTarget` is already null, so routing
		`move_hover` through one leaves the pill parked forever.
	-->
	<div
		role="presentation"
		class="flex min-w-0 items-center gap-1"
		onpointerover={move_hover}
		onpointermove={move_hover}
		onfocusin={move_hover}
		onpointerenter={yield* PreviewMode(input.mode)}
	>
		<button
			type="button"
			disabled={!forge_available}
			aria-current={input.active ? "true" : undefined}
			class="flex min-w-0 grow items-center gap-2 rounded-xl px-2.5 py-1.5 text-left focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
			onclick={yield* SelectSelection(
				input.mode === "curated" ? { _tag: "Curated" } : { _tag: "Inherited" },
			)}
			onfocus={yield* PreviewMode(input.mode)}
		>
			{@render barekey_mark("size-5")}
			<span class="flex min-w-0 flex-col space-y-0">
				<span class="truncate text-sm font-semibold text-foreground">{input.name}</span>
				<span class="truncate text-xs text-muted-foreground">{input.lab}</span>
			</span>
		</button>
	</div>
{/snippet}

<!--
	A settings control wears the same well as the composer's policy row: a
	gradient card holding a bare trigger, opening onto glass.
-->
{#snippet policy_select(label, value, options, onchoose)}
	<label class="flex min-w-0 flex-col gap-1 text-xs text-muted-foreground">
		{label}
		<Select type="single" {value} onValueChange={yield* onchoose(event)} disabled={!forge_available}>
			<div
				class="card min-w-0 rounded-md bg-linear-to-b from-surface-225 to-surface-200 dark:from-surface-800 dark:to-surface-925"
			>
				<SelectTrigger
					size="sm"
					class="h-6 w-full border-transparent bg-transparent px-2 text-xs shadow-none data-[size=sm]:h-6 dark:bg-transparent dark:hover:bg-transparent dark:hover:text-foreground"
				>
					<span class="truncate">
						{options.find((option) => option.id === value)?.label ?? label}
					</span>
				</SelectTrigger>
			</div>
			<SelectContent class="rounded-2xl border-transparent bg-transparent p-0 shadow-none">
				<ShaderGlassSurface strength="strong" class="rounded-2xl p-1">
					<DropdownHoverSurface class="[--docs-sidebar-hover-radius:var(--radius-xl)]">
						{#snippet children({ move_hover })}
							{#each options as option (option.id)}
								<SelectItem
									value={option.id}
									class="focus:bg-transparent! data-highlighted:bg-transparent! data-highlighted:text-foreground!"
									{@attach FollowHighlight(move_hover)}
								>
									{option.label}
								</SelectItem>
							{/each}
						{/snippet}
					</DropdownHoverSurface>
				</ShaderGlassSurface>
			</SelectContent>
		</Select>
	</label>
{/snippet}

{#snippet compaction_model_picker()}
	<Popover bind:open={picker_open}>
		<PopoverTrigger
			aria-label="Cross-transfer compaction model"
			disabled={!forge_available}
			class="card flex h-7 w-full max-w-full shrink-0 items-center gap-2 rounded-md bg-linear-to-b from-surface-225 to-surface-200 px-2.5 text-left text-xs text-foreground outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:ring-inset disabled:pointer-events-none disabled:opacity-45 sm:w-auto sm:max-w-72 dark:from-surface-800 dark:to-surface-925"
		>
			{#if TriggerIcon === undefined}
				{@render barekey_mark("size-3.5")}
			{:else}
				<TriggerIcon class={trigger_monochrome ? "size-3.5 shrink-0 dark:invert" : "size-3.5 shrink-0 text-muted-foreground"} />
			{/if}
			<span class="truncate">{trigger_label}</span>
			<Selector class="pointer-events-none size-3.5 shrink-0 text-muted-foreground" />
		</PopoverTrigger>
		<PopoverContent
			variant="bare"
			align="end"
			side="bottom"
			sideOffset={8}
			class="w-[min(30rem,calc(100vw-2rem))] rounded-3xl"
		>
			<ShaderGlassSurface strength="strong" class="w-full rounded-3xl">
				<Tabs bind:value={active_engine} class="min-h-0 gap-2 p-2">
					<EngineSection {active_engine} disabled={!forge_available} engine_locked={false} {engines} selected_engine={active_engine_choice} />
					<div class="flex min-w-0 flex-col gap-2 sm:flex-row">
						<div
							class="docs-scroll-fade h-48 min-w-0 w-full grow overflow-y-auto rounded-xl"
						>
							<ModelList
								disabled={!forge_available}
								{favorite_ids}
								favorites_available={forge_available}
								models={active_models}
								onfavorite={RequestFavorite}
								onpreview={PreviewModelId}
								onselect={SelectModel}
								selected_model_id={compaction_model?.id ?? ""}
							>
								{#snippet leading({ move_hover })}
									{@render mode_row(
										{
											active: !inherited && compaction_model === undefined,
											lab: "Artisan",
											mode: "curated",
											name: "Curated",
										},
										move_hover,
									)}
									{@render mode_row(
										{ active: inherited, lab: "Thread model", mode: "inherited", name: "Inherited" },
										move_hover,
									)}
								{/snippet}
							</ModelList>
						</div>
						<div class="min-h-28 w-full shrink-0 border-t border-border/40 sm:h-48 sm:w-56 sm:border-t-0">
							<div class="flex h-full flex-col justify-between gap-2 overflow-y-auto p-2.5">
							{#if previewed_pane === "curated"}
								<div class="flex min-w-0 flex-col gap-3">
									<span class="text-pretty text-xs text-muted-foreground">
										A curated, cost-effective model for each engine.
									</span>
									<!-- Each engine wears its own mark, and the model it resolves to is the answer, so it carries the foreground. -->
									<div class="flex min-w-0 flex-col gap-1">
										{#each curated_rows as row (row.engine.id)}
											{@const EngineIcon = row.engine.icon}
											<span class="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
												<EngineIcon class={EngineMarkClass(row.engine, "size-3.5")} />
												<span class="shrink-0">{row.engine.name} —</span>
												<span class="min-w-0 truncate text-foreground">{row.model_name}</span>
											</span>
										{/each}
									</div>
								</div>
							{:else if previewed_pane === "inherited"}
								<span class="text-pretty text-xs text-muted-foreground">
									The thread's current model writes its own hand-off summary.
								</span>
							{:else if previewed_model !== undefined}
								<div class="flex min-w-0 flex-col gap-1">
									<span class="truncate text-sm font-semibold text-foreground">
										{previewed_model.name}
									</span>
									{#if previewed_model.definition.description !== undefined}
										<span class="text-pretty text-xs text-muted-foreground">
											{previewed_model.definition.description}
										</span>
									{/if}
								</div>
								<div class="flex flex-col gap-1.5">
									{#if previewed_model.definition.capabilities.thinking.availability === "supported"}
										{@render policy_select(
											"Reasoning",
											ThinkingForDefaults(defaults, previewed_model) ?? "",
											previewed_model.definition.capabilities.thinking.availability === "supported"
												? previewed_model.definition.capabilities.thinking.options.map((option) => ({
														id: option.id,
														label: thinking_level_labels[option.id] ?? option.id,
													}))
												: [],
											UpdateThinking,
										)}
									{/if}
									{#if ContextForDefaults(defaults, previewed_model) !== undefined}
										{@render policy_select(
											"Context window",
											ContextForDefaults(defaults, previewed_model)?.id ?? "",
											(previewed_model.definition.capabilities.context_window?.options ?? []).map(
												(option) => ({ id: option.id, label: option.label }),
											),
											UpdateContext,
										)}
									{/if}
									{#if previewed_permissions.length > 1}
										{@render policy_select(
											"Permission",
											defaults.permission,
											previewed_permissions.map((option) => ({ id: option.id, label: option.label })),
											UpdatePermission,
										)}
									{/if}
								</div>
							{/if}
							</div>
						</div>
					</div>
				</Tabs>
			</ShaderGlassSurface>
		</PopoverContent>
	</Popover>
{/snippet}

<div class="card rounded-xl bg-linear-to-b from-surface-225 to-surface-200 dark:from-surface-800 dark:to-surface-925">
	<div class="flex flex-col items-stretch justify-between gap-3 px-4 py-3.5 sm:flex-row sm:items-center sm:gap-6">
		<div class="flex min-w-0 flex-col gap-0.5">
			<span class="flex items-center gap-1.5 text-sm text-foreground"><ArrowsMinimize class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />Cross-transfer compaction model</span>
			<span class="text-pretty text-xs text-muted-foreground">Choose who writes a hand-off summary when a thread moves to another engine or model.</span>
		</div>
		{@render compaction_model_picker()}
	</div>
</div>
