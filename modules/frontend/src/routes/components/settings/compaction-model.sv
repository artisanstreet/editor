<script lang="ts" effect>
	import ArrowsMinimize from "@tabler/icons-svelte/icons/arrows-minimize";
	import Selector from "@tabler/icons-svelte/icons/selector";
	import Tool from "@tabler/icons-svelte/icons/tool";
	import type { Component } from "svelte";
	import { Effect, Stream } from "effect";
	import type { SessionDefaults } from "@artisan/protocol";
	import { BannerService } from "$lib/banner/service";
	import barekey_logo from "$lib/assets/barekey/logo-40.png";
	import { EngineMarkFor } from "$lib/engine/presentation";
	import {
		CompactionSelectionFromDefaults,
		SessionDefaultsController,
		type CompactionSelection,
		type SessionDefaultsState,
	} from "$lib/settings/session-defaults-controller";
	import {
		ModelsFromCatalog,
		type EngineChoice,
		type HarnessId,
		type ModelChoice,
		type PermissionOption,
	} from "$lib/engine/model-selection";
	import { Popover, PopoverContent, PopoverTrigger } from "$lib/components/ui/popover";
	import { Tabs } from "$lib/components/ui/tabs";
	import EngineSection from "../model-selector/engine-section.sv";
	import {
		ContextForDefaults,
		ModelsForEngine,
		PermissionsForSelection,
		ThinkingForDefaults,
	} from "../model-selector/presentation";
	import ShaderGlassSurface from "../shader-glass-surface.sv";

	const banner = yield* BannerService;
	const defaults_controller = yield* SessionDefaultsController;
	const initial = yield* defaults_controller.Refresh.pipe(
		Effect.catch((error) =>
			Effect.gen(function* () {
				yield* banner.error("Could not load compaction defaults", {
					description: error.message,
				});
				return yield* defaults_controller.Current;
			}),
		),
	);
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
		`mask-image: url(${barekey_logo})`,
		"mask-size: contain",
		"mask-repeat: no-repeat",
		"mask-position: center",
		`-webkit-mask-image: url(${barekey_logo})`,
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
	const active_models = $derived(ModelsForEngine(models, active_engine));
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

	const LoadDefaults = Effect.gen(function* () {
		const next = yield* defaults_controller.Refresh;
		yield* ApplyDefaults(next);
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
			Effect.catch((error) =>
				Effect.gen(function* () {
					yield* banner.error("Could not save compaction defaults", {
						description: error.message,
					});
					yield* LoadDefaults.pipe(
						Effect.catch(() =>
							Effect.gen(function* () {
							}),
						),
					);
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

	const UpdateThinking = (event: Event) =>
		Effect.gen(function* () {
			if (!(event.currentTarget instanceof HTMLSelectElement) || previewed_model === undefined) return;
			const capability = previewed_model.definition.capabilities.thinking;
			if (capability.availability !== "supported") return;
			const level = capability.options.find((option) => option.id === event.currentTarget.value)?.id;
			if (level === undefined) return;
			yield* SaveDefaults(
				{ _tag: "Explicit", model_id: previewed_model.id },
				{ model: {
					model_id: previewed_model.id,
					reasoning_effort: level === "light" ? "low" : level,
				} },
			);
		});

	const UpdateContext = (event: Event) =>
		Effect.gen(function* () {
			if (!(event.currentTarget instanceof HTMLSelectElement) || previewed_model === undefined) return;
			const context = previewed_model.definition.capabilities.context_window;
			const option = context?.options.find((candidate) => candidate.id === event.currentTarget.value);
			if (option === undefined) return;
			yield* SaveDefaults(
				{ _tag: "Explicit", model_id: previewed_model.id },
				{ model: {
					...(option.native_suffix === "" ? {} : { context_window: option.native_suffix }),
					model_id: previewed_model.id,
				} },
			);
		});

	const UpdatePermission = (event: Event) =>
		Effect.gen(function* () {
			if (!(event.currentTarget instanceof HTMLSelectElement) || previewed_model === undefined) return;
			const option = previewed_permissions.find((candidate) => candidate.id === event.currentTarget.value);
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

{#snippet mode_row(input)}
	<button
		type="button"
		disabled={!forge_available}
		aria-current={input.active ? "true" : undefined}
		class="flex w-full min-w-0 items-center gap-2 rounded-xl px-2.5 py-1.5 text-left text-sm focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
		onpointerenter={yield* PreviewMode(input.mode)}
		onclick={yield* SelectSelection(
			input.mode === "curated" ? { _tag: "Curated" } : { _tag: "Inherited" },
		)}
	>
		{@render barekey_mark("size-5")}
		<span class="flex min-w-0 flex-col">
			<span class="truncate font-semibold text-foreground">{input.name}</span>
			<span class="truncate text-xs text-muted-foreground">{input.lab}</span>
		</span>
	</button>
{/snippet}

{#snippet compaction_model_picker()}
	<Popover bind:open={picker_open}>
		<PopoverTrigger
			aria-label="Cross-transfer compaction model"
			disabled={!forge_available}
			class="card flex h-7 shrink-0 items-center gap-2 rounded-md bg-linear-to-b from-surface-225 to-surface-200 px-2.5 text-left text-xs text-foreground outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:ring-inset disabled:pointer-events-none disabled:opacity-45 dark:from-surface-800 dark:to-surface-925"
		>
			{#if TriggerIcon === undefined}
				{@render barekey_mark("size-3.5")}
			{:else}
				<TriggerIcon class={trigger_monochrome ? "size-3.5 shrink-0 dark:invert" : "size-3.5 shrink-0 text-muted-foreground"} />
			{/if}
			<span class="truncate">{trigger_label}</span>
			<Selector class="pointer-events-none size-3.5 shrink-0 text-muted-foreground" />
		</PopoverTrigger>
		<PopoverContent variant="bare" align="end" side="bottom" sideOffset={8} class="w-[min(30rem,calc(100vw-2rem))] rounded-3xl">
			<ShaderGlassSurface strength="strong" class="w-full rounded-3xl">
				<Tabs bind:value={active_engine} class="min-h-0 gap-2 p-2">
					<EngineSection {active_engine} disabled={!forge_available} engine_locked={false} {engines} selected_engine={active_engine_choice} />
					<div class="flex min-w-0 gap-2">
						<div class="docs-scroll-fade h-56 min-w-0 grow overflow-y-auto rounded-xl [scrollbar-width:thin]">
							<div class="flex flex-col gap-0.5 p-1.5">
								{@render mode_row({ active: !inherited && compaction_model === undefined, lab: "Artisan", mode: "curated", name: "Curated" })}
								{@render mode_row({ active: inherited, lab: "Thread model", mode: "inherited", name: "Inherited" })}
								{#each active_models as model (model.id)}
									<button type="button" disabled={!forge_available || model.definition.disabled !== undefined} aria-current={model.id === compaction_model?.id ? "true" : undefined} class="flex min-w-0 items-center gap-2 rounded-xl px-2.5 py-1.5 text-left focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45" onpointerenter={yield* PreviewModel(model)} onclick={yield* SelectModel(model)}>
										<span class="min-w-0 truncate text-sm font-semibold text-foreground">{model.name}</span>
										<span class="ml-auto shrink-0 text-xs text-muted-foreground">{model.lab}</span>
									</button>
								{/each}
							</div>
						</div>
						<div class="h-56 w-56 shrink-0 overflow-y-auto p-2.5">
							{#if previewed_pane === "curated"}
								<p class="text-xs text-muted-foreground">A curated, cost-effective model for each engine.</p>
								{#each curated_rows as row (row.engine.id)}<p class="mt-1 text-xs text-muted-foreground">{row.engine.name} — {row.model_name}</p>{/each}
							{:else if previewed_pane === "inherited"}
								<p class="text-xs text-muted-foreground">The thread's current model writes its own hand-off summary.</p>
							{:else if previewed_model !== undefined}
								<p class="text-sm font-semibold text-foreground">{previewed_model.name}</p>
								{#if previewed_model.definition.capabilities.thinking.availability === "supported"}
									<label class="mt-3 block text-xs text-muted-foreground">Reasoning
						<select class="mt-1 w-full rounded-md border border-input bg-background px-2 py-1 text-foreground" value={ThinkingForDefaults(defaults, previewed_model)} onchange={yield* UpdateThinking(event)}>
											{#each previewed_model.definition.capabilities.thinking.options as option (option.id)}<option value={option.id}>{option.id}</option>{/each}
										</select>
									</label>
								{/if}
								{#if ContextForDefaults(defaults, previewed_model) !== undefined}
									<label class="mt-3 block text-xs text-muted-foreground">Context window
						<select class="mt-1 w-full rounded-md border border-input bg-background px-2 py-1 text-foreground" value={ContextForDefaults(defaults, previewed_model)?.id} onchange={yield* UpdateContext(event)}>
											{#each previewed_model.definition.capabilities.context_window?.options ?? [] as option (option.id)}<option value={option.id}>{option.label}</option>{/each}
										</select>
									</label>
								{/if}
								{#if previewed_permissions.length > 1}
									<label class="mt-3 block text-xs text-muted-foreground">Permission
										<select class="mt-1 w-full rounded-md border border-input bg-background px-2 py-1 text-foreground" value={defaults.permission} onchange={yield* UpdatePermission(event)}>
											{#each previewed_permissions as option (option.id)}<option value={option.id}>{option.label}</option>{/each}
										</select>
									</label>
								{/if}
							{/if}
						</div>
					</div>
				</Tabs>
			</ShaderGlassSurface>
		</PopoverContent>
	</Popover>
{/snippet}

<div class="card rounded-xl bg-linear-to-b from-surface-225 to-surface-200 dark:from-surface-800 dark:to-surface-925">
	<div class="flex items-center justify-between gap-6 px-4 py-3.5">
		<div class="flex min-w-0 flex-col gap-0.5">
			<span class="flex items-center gap-1.5 text-sm text-foreground"><ArrowsMinimize class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />Cross-transfer compaction model</span>
			<span class="text-pretty text-xs text-muted-foreground">Choose who writes a hand-off summary when a thread moves to another engine or model.</span>
		</div>
		{@render compaction_model_picker()}
	</div>
</div>
