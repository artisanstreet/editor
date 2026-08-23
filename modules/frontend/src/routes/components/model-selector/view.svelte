<script lang="ts" effect>
	import Selector from "@tabler/icons-svelte/icons/selector";
	import { Effect, Stream } from "effect";
	import { untrack } from "svelte";
	import {
		SessionPolicyPermission,
		type RuntimeCatalog,
		type ThreadSessionPolicy,
	} from "@artisan/protocol";
	import { EngineMarkClass, EngineMarkFor } from "$lib/engine/presentation";
	import { speed_option_presentation } from "$lib/engine/speed-presentation";
	import {
		SessionDefaultsController,
		type SessionDefaultsState,
	} from "$lib/settings/session-defaults-controller";
	import {
		IsOfflineRuntimeCatalog,
		OfflineRuntimeCatalog,
	} from "$lib/runtime/offline-catalog";

	import { Popover, PopoverContent, PopoverTrigger } from "$lib/components/ui/popover";
	import { Tabs } from "$lib/components/ui/tabs";
	import {
		Tooltip,
		TooltipContent,
		TooltipProvider,
		TooltipTrigger,
	} from "$lib/components/ui/tooltip";
	import ShaderGlassSurface from "../shader-glass-surface.svelte";
	import EngineSection from "./engine-section.svelte";
	import ModelList from "./model-list.svelte";
	import ModelPreviewSummary from "./model-preview-summary.svelte";
	import {
		MakeModelPolicyController,
		type PolicyFlushResult,
	} from "./policy-controller";
	import PolicyControls from "./policy-controls.svelte";
	import {
		ModelsFromCatalog,
		OrderModels,
		PermissionsForModel,
		permission_for_harness,
		permission_policy_for_harness,
		permission_reconciliation_for_harness,
		policy_fields_for_permission,
		RouteGroupsForModels,
		VariantLabel,
		VariantsForModel,
		type ContextWindowChoice,
		type EngineChoice,
		type HarnessId,
		type ModelChoice,
		type PermissionOption,
		type SpeedOption,
		type ThinkingLevel,
		thinking_level_labels,
	} from "$lib/engine/model-selection";

	type ModelFavoriteRequest = { readonly favorite: boolean; readonly model_id: string };

	let {
		disabled = false,
		onpolicychange,
		policy,
		runtime_catalog,
	}: {
		disabled?: boolean;
		onpolicychange?: (
			policy: ThreadSessionPolicy,
		) => Effect.Effect<ThreadSessionPolicy, { readonly message: string }>;
		policy?: ThreadSessionPolicy;
		runtime_catalog: RuntimeCatalog;
	} = $props();

	const defaults_controller = yield* SessionDefaultsController;
	/** The shell owns Forge hydration; selectors paint from its retained snapshot. */
	let defaults_state = $state.raw<SessionDefaultsState | undefined>(
		yield* defaults_controller.Current,
	);
	const effective_catalog = $derived(defaults_state?.catalog ?? runtime_catalog);
	const model_manifest = $derived(effective_catalog.manifest);
	/**
	 * The blanket availability switch, honoured at the selector: a disabled
	 * engine's section and models are not represented at all, rather than
	 * shown greyed — the switch means "this engine does not exist here".
	 */
	const disabled_engines = $derived(
		new Set(defaults_state?.defaults.disabled_engines ?? []),
	);
	const engines: ReadonlyArray<EngineChoice> = $derived(
		model_manifest.harnesses
			.filter((harness) => !disabled_engines.has(harness.id))
			.map((harness) => ({
				id: harness.id,
				name: harness.label,
				...EngineMarkFor(harness.id),
			})),
	);

	const models = $derived(
		ModelsFromCatalog(effective_catalog).filter(
			(model) => !disabled_engines.has(model.engine),
		),
	);
	const initial_models = ModelsFromCatalog(OfflineRuntimeCatalog);
	let open = $state(false);
	let previewed_model_id = $state<string | undefined>(undefined);
	let thinking_level = $state<ThinkingLevel>("medium");
	let speed_option_id = $state("standard");
	let active_engine = $state<HarnessId>(initial_models[0]?.engine ?? "codex");
	let permission_mode = $state("autonomous");
	let selected_model_id = $state(
		OfflineRuntimeCatalog.default_model_id ?? initial_models[0]?.id ?? "",
	);
	/**
	 * Forge owns the starred set, so every client opens the picker to the same
	 * order. It is read as its own statement rather than an awaited binding: a
	 * favorite is a nicety, and the picker must not wait on it to render.
	 */
	let favorite_ids = $state.raw<ReadonlyArray<string>>(defaults_state.favorite_ids);
	const favorites_available = $derived(
		defaults_state?.available ?? !IsOfflineRuntimeCatalog(effective_catalog),
	);
	const policy_controller = yield* MakeModelPolicyController;
	let displayed_policy = $state.raw<ThreadSessionPolicy | undefined>(undefined);
	const PersistFavorite = (request: ModelFavoriteRequest) =>
		Effect.gen(function* () {
			const snapshot = yield* defaults_controller.SetFavorite(
				request.model_id,
				request.favorite,
			);
			favorite_ids = snapshot.favorite_ids;
		}).pipe(Effect.catch(() => Effect.void));

	const RequestFavorite = (model_id: string, favorite: boolean) =>
		Effect.gen(function* () {
			if (disabled || !favorites_available) return;
			favorite_ids = favorite
				? [...favorite_ids.filter((id) => id !== model_id), model_id]
				: favorite_ids.filter((id) => id !== model_id);
			yield* PersistFavorite({ favorite, model_id });
		});

	const PreviewModel = (model_id: string) =>
		Effect.gen(function* () {
			previewed_model_id = model_id;
		});

	const ApplyDefaultsChange = (snapshot: SessionDefaultsState) =>
		Effect.gen(function* () {
			defaults_state = snapshot;
			favorite_ids = snapshot.favorite_ids;
		});

	yield* defaults_controller.Changes.pipe(
		Stream.runForEach(ApplyDefaultsChange),
		Effect.forkScoped,
	);

	const ThinkingLevelFromPolicy = (
		effort: ThreadSessionPolicy["reasoning_effort"],
	): ThinkingLevel => (effort === "low" ? "light" : effort);
	const PolicyEffortFromThinking = (
		level: ThinkingLevel,
	): ThreadSessionPolicy["reasoning_effort"] =>
		level === "light" ? "low" : level;
	const PermissionModeFromPolicy = SessionPolicyPermission;
	/**
	 * Persists the Forge-owned half of a policy change. Permission is shared
	 * across every model and engine; effort, speed, and context belong to the model
	 * that was configured, because their option sets differ per model. The write is
	 * deliberately secondary to the thread's authoritative policy: failure is
	 * reported, but it cannot roll back a thread policy Forge already accepted.
	 */
	const RememberDefaults = (next: ThreadSessionPolicy) =>
		Effect.gen(function* () {
			yield* defaults_controller.RememberPolicyDefaults(next).pipe(
				Effect.catch(() => Effect.void),
			);
		});

	const PersistPolicy = (desired: ThreadSessionPolicy) =>
		Effect.gen(function* () {
			const persist = onpolicychange;
			if (persist === undefined) {
				return yield* Effect.fail({ message: "Thread policy persistence is unavailable" });
			}
			return yield* persist(desired);
		});

	const FlushPolicy = Effect.gen(function* () {
		const result = yield* policy_controller.Flush(PersistPolicy).pipe(
			Effect.catchTag("ModelPolicyMutationError", () =>
				Effect.gen(function* () {
					return {
						confirmed: [],
						current: yield* policy_controller.Current,
					} satisfies PolicyFlushResult;
				}),
			),
		);
		displayed_policy = result.current;
		for (const confirmed of result.confirmed) yield* RememberDefaults(confirmed);
	});

	const ReplacePolicy = (next: ThreadSessionPolicy) =>
		Effect.gen(function* () {
			displayed_policy = yield* policy_controller.Replace(next);
		});

	const PatchPolicy = (patch: Partial<ThreadSessionPolicy>) =>
		Effect.gen(function* () {
			displayed_policy = yield* policy_controller.Patch(patch);
		});

	/**
	 * Favorites float to the top of the engine they belong to, in the order
	 * Forge stores them, and everything else keeps its catalog order. Sorting
	 * within the engine rather than across engines keeps the tab and the list
	 * agreeing about what is on screen.
	 */
	const active_models = $derived.by(() => {
		return OrderModels(models, active_engine, favorite_ids, selected_model_id);
	});
	const route_groups = $derived(
		RouteGroupsForModels(effective_catalog, active_engine, active_models),
	);
	const selected_policy = $derived(displayed_policy ?? policy);
	const selected_model = $derived(
		models.find((model) => model.id === selected_model_id) ??
			(selected_policy === undefined ? models[0] : undefined),
	);
	const selected_engine = $derived(
		selected_model?.engine ?? selected_policy?.engine_id ?? active_engine,
	);
	/**
	 * The speed only earns a word when it is not the model's own default: every
	 * model would otherwise trail a "Standard" that says nothing.
	 */
	const trigger_speed_presentation = $derived.by(() => {
		const speeds = selected_model?.definition.capabilities.speed_options ?? [];
		const selected = speeds.find((option) => option.id === speed_option_id);
		if (selected === undefined || selected.default) return undefined;
		return speed_option_presentation(selected);
	});
	const selected_thinking_level = $derived(
		selected_model?.definition.capabilities.thinking.availability === "supported"
			? thinking_level
			: undefined,
	);
	const selected_variant = $derived(
		selected_model?.definition.native_selection?.variant_id === undefined
			? undefined
			: VariantLabel(selected_model),
	);
	const selected_harness = $derived(
		model_manifest.harnesses.find((harness) => harness.id === selected_engine),
	);
	const selected_permission_options = $derived(selected_harness?.permissions.options ?? []);
	const previewed_model = $derived(
		models.find((model) => model.id === previewed_model_id) ?? selected_model,
	);
	const previewed_variants = $derived(
		previewed_model === undefined ? [] : VariantsForModel(models, previewed_model),
	);
	const selected_permissions = $derived(
		selected_model === undefined
			? undefined
			: PermissionsForModel(effective_catalog, selected_model),
	);
	/**
	 * The controls describe the previewed model, so its harness decides which
	 * permission options they may offer. Reading the selection's here instead
	 * would let a Claude row be configured with Codex's vocabulary whenever the
	 * two harnesses disagree.
	 */
	const previewed_permissions = $derived(
		previewed_model === undefined
			? undefined
			: PermissionsForModel(effective_catalog, previewed_model),
	);
	/** The only way the whole selector disables: the thread session is not connected. */
	const disabled_reason = $derived(
		disabled ? "Unavailable until the thread's session is connected" : undefined,
	);

	/** Makes the model current without flushing so inline controls coalesce with it. */
	const AdoptModel = (model: ModelChoice) =>
		Effect.gen(function* () {
			if (model.definition.disabled !== undefined) return false;

			selected_model_id = model.id;
			active_engine = model.engine;
			if (model.definition.capabilities.thinking.availability === "supported") {
				thinking_level = model.definition.capabilities.thinking.default;
			}
			const default_speed =
				model.definition.capabilities.speed_options.find(
					(option) => option.default && option.disabled === undefined,
				) ??
				model.definition.capabilities.speed_options.find(
					(option) => option.disabled === undefined,
				);
			speed_option_id = default_speed?.id ?? "standard";
			const target_permission = permission_for_harness(
				effective_catalog,
				model.engine,
				permission_mode,
			);
			if (target_permission !== undefined) permission_mode = target_permission.id;

			const base_policy = yield* policy_controller.Current;
			if (disabled || base_policy === undefined || onpolicychange === undefined) return true;
			const {
				catalog_revision: _catalog_revision,
				context_window: _reset,
				model_id: _model_id,
				profile_id: _profile_id,
				provider_route_id: _provider_route_id,
				variant_id: _variant_id,
				...rest
			} = base_policy;
			const native_selection = model.definition.native_selection;
			yield* ReplacePolicy({
				...rest,
				...policy_fields_for_permission(target_permission),
				...(native_selection === undefined
					? {}
					: {
							catalog_revision: effective_catalog.catalog_revision ?? effective_catalog.manifest.revision,
							model_id: native_selection.model_id,
							profile_id: effective_catalog.scope?.profile_id ?? "default",
							provider_route_id: native_selection.provider_route_id,
							...(native_selection.variant_id === undefined
								? {}
								: { variant_id: native_selection.variant_id }),
						}),
				engine_id: model.engine,
				model: model.definition.native_model_id,
				reasoning_effort:
					model.definition.capabilities.thinking.availability === "supported"
						? PolicyEffortFromThinking(model.definition.capabilities.thinking.default)
						: base_policy.reasoning_effort,
				service_tier: default_speed?.native_value ?? "standard",
			});
			return true;
		});

	/**
	 * Turns a previewed model into the configured one at the moment its settings
	 * are actually touched.
	 *
	 * Hovering alone still adopts nothing — the pointer passing over a row must
	 * not rewrite the thread's policy. But a setting is a statement about the
	 * model it sits under, and there is no way to hold an effort or a window for
	 * a model the thread is not using, so the first control the reader touches
	 * is the moment the preview becomes the choice.
	 */
	const AdoptForConfiguration = (model: ModelChoice) =>
		Effect.gen(function* () {
			if (selected_model?.id === model.id) return true;
			return yield* AdoptModel(model);
		});

	const ApplyModelContext = (model: ModelChoice, option: ContextWindowChoice) =>
		Effect.gen(function* () {
			if (!(yield* AdoptForConfiguration(model))) return;
			const base_policy = yield* policy_controller.Current;
			if (disabled || base_policy === undefined || onpolicychange === undefined) return;
			const { context_window: _previous, ...rest } = base_policy;
			yield* ReplacePolicy(
				option.native_suffix === "" ? rest : { ...rest, context_window: option.native_suffix },
			);
			yield* FlushPolicy;
		});

	const SelectModel = (model: ModelChoice) =>
		Effect.gen(function* () {
			if (!(yield* AdoptModel(model))) return;
			open = false;
			yield* FlushPolicy;
		});

	const ApplyModelThinking = (model: ModelChoice, level: ThinkingLevel) =>
		Effect.gen(function* () {
			if (!(yield* AdoptForConfiguration(model))) return;
			thinking_level = level;
			yield* PatchPolicy({ reasoning_effort: PolicyEffortFromThinking(level) });
			yield* FlushPolicy;
		});

	const ApplyModelSpeed = (model: ModelChoice, option: SpeedOption) =>
		Effect.gen(function* () {
			if (option.disabled !== undefined) return;
			if (!(yield* AdoptForConfiguration(model))) return;
			speed_option_id = option.id;
			yield* PatchPolicy({ service_tier: option.native_value });
			yield* FlushPolicy;
		});

	const ApplyModelPermission = (model: ModelChoice, option: PermissionOption) =>
		Effect.gen(function* () {
			if (!(yield* AdoptForConfiguration(model))) return;
			permission_mode = option.id;
			yield* PatchPolicy(policy_fields_for_permission(option));
			yield* FlushPolicy;
		});

	const ApplyModelVariant = (model: ModelChoice) =>
		Effect.gen(function* () {
			if (!(yield* AdoptModel(model))) return;
			previewed_model_id = model.id;
			yield* FlushPolicy;
		});

	const SyncAuthoritativePolicy = (next: ThreadSessionPolicy) =>
		Effect.gen(function* () {
			displayed_policy = yield* policy_controller.SetAuthoritative(next);
			const current = displayed_policy ?? next;
			const model = models.find((candidate) => {
				if (current.model === undefined)
					return candidate.id === effective_catalog.default_model_id;
				if (
					candidate.engine !== current.engine_id ||
					candidate.definition.native_model_id !== current.model
				)
					return false;
				const selection = candidate.definition.native_selection;
				return selection === undefined
					? current.provider_route_id === undefined && current.variant_id === undefined
					: selection.provider_route_id === current.provider_route_id &&
						selection.model_id === (current.model_id ?? current.model) &&
						selection.variant_id === current.variant_id;
			});
			if (model !== undefined && model.id !== untrack(() => selected_model_id)) {
				selected_model_id = model.id;
				active_engine = model.engine;
			}
			thinking_level = ThinkingLevelFromPolicy(current.reasoning_effort);
			permission_mode = PermissionModeFromPolicy(current);
			const speed_option =
				model?.definition.capabilities.speed_options.find(
					(option) => option.native_value === current.service_tier,
				) ?? model?.definition.capabilities.speed_options.find((option) => option.default);
			speed_option_id = speed_option?.id ?? "standard";
		});

	if (policy !== undefined) yield* SyncAuthoritativePolicy(policy);

	const ResetPreview = Effect.gen(function* () {
		previewed_model_id = undefined;
	});
	if (!open && previewed_model_id !== undefined) yield* ResetPreview;

	const ReconcilePermission = Effect.gen(function* () {
		if (selected_permission_options.length === 0) return;
		const engine = selected_engine;
		const current = displayed_policy ?? policy;
		const resolved =
			current === undefined
				? {
						...permission_policy_for_harness(effective_catalog, engine, permission_mode),
						needs_update: false,
					}
				: permission_reconciliation_for_harness(effective_catalog, engine, current);
		if (permission_mode !== resolved.fields.permission) {
			permission_mode = resolved.fields.permission;
		}
		if (current === undefined || onpolicychange === undefined || !resolved.needs_update) return;
		const requested = yield* policy_controller.RequestRepair({ ...current, ...resolved.fields });
		if (requested) yield* FlushPolicy;
	});
	if (selected_permission_options.length > 0) yield* ReconcilePermission;
</script>

<TooltipProvider delayDuration={0} ignoreNonKeyboardFocus>
<div class="no-scrollbar flex min-w-0 max-w-full items-center gap-1 overflow-x-auto">
	<Popover bind:open>
		<Tooltip>
			<TooltipTrigger>
				{#snippet child({ props: tooltip_props })}
					<span {...tooltip_props} class="flex min-w-0 has-[:disabled]:cursor-not-allowed">
					<PopoverTrigger
							aria-label="Select model"
							disabled={disabled}
							class="model-trigger group/model-trigger flex h-8 shrink-0 items-center gap-2 rounded-sm bg-transparent px-2 text-left text-foreground outline-none transition-colors focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:ring-inset disabled:pointer-events-none"
						>
							{@const trigger_mark = EngineMarkFor(selected_engine)}
							{@const TriggerMark = trigger_mark.icon}
							<span class="flex min-w-0 items-center gap-2">
								<TriggerMark class={EngineMarkClass(trigger_mark, "size-4")} />
								<span class="flex min-w-0 items-center gap-1 whitespace-nowrap text-sm">
									<span class="text-foreground">
										{selected_model?.name ?? selected_policy?.model ?? "No model"}
									</span>
									{#if selected_thinking_level !== undefined}
										<span class="text-muted-foreground">
											{thinking_level_labels[selected_thinking_level]}
										</span>
									{/if}
									{#if selected_variant !== undefined}
										<span class="text-muted-foreground">{selected_variant}</span>
									{/if}
									{#if trigger_speed_presentation !== undefined}
										<span class={trigger_speed_presentation.class_name}>
											{trigger_speed_presentation.label}
										</span>
									{/if}
								</span>
							</span>
							<Selector class="pointer-events-none size-3.5 shrink-0 text-muted-foreground" />
						</PopoverTrigger>
					</span>
				{/snippet}
			</TooltipTrigger>
			{#if disabled_reason !== undefined}
				<TooltipContent>{disabled_reason}</TooltipContent>
			{/if}
		</Tooltip>

		<!--
			Anchored to the trigger's leading edge, not its trailing one: the label
			changes width as the model, effort and speed change, so an end-aligned
			card slid sideways every time a value was picked.
		-->
		<PopoverContent
			variant="bare"
			align="start"
			side="top"
			sideOffset={8}
			data-strength="strong"
			class="t-dropdown shader-glass-backdrop w-[min(30rem,calc(100vw-2rem))] rounded-3xl animate-none!"
		>
			<ShaderGlassSurface
				strength="strong"
				class="t-resize t-resize-auto w-full rounded-3xl"
				use_backdrop_filter={false}
			>
				<Tabs bind:value={active_engine} class="min-h-0 gap-2 p-2">
				<EngineSection
					{active_engine}
					{disabled}
					{disabled_reason}
					{engines}
				/>
				<div class="flex min-w-0 gap-2">
					<div class="docs-scroll-fade h-48 min-w-0 grow overflow-y-auto">
						<ModelList
							{disabled}
							{favorite_ids}
							{favorites_available}
							models={active_models}
							onfavorite={RequestFavorite}
							onpreview={PreviewModel}
							onselect={SelectModel}
							{route_groups}
							{selected_model_id}
						/>
					</div>
					{#if previewed_model !== undefined && selected_model !== undefined}
						<!--
							The preview deliberately survives the pointer arriving here.
							Clearing it on enter snapped the whole panel back to the current
							selection the instant you moved toward the controls, so the only
							settings you could ever reach were the ones you already had —
							the hovered model's could not be touched without selecting it
							first, which is the click this panel exists to save.
						-->
						<div class="h-48 w-56 shrink-0" role="presentation">
							<div class="flex h-full flex-col justify-between gap-2 overflow-y-auto p-2.5">
								<ModelPreviewSummary model={previewed_model} />
								<PolicyControls
									{disabled}
									model={previewed_model}
									oncontext={ApplyModelContext}
									onpermission={ApplyModelPermission}
									onspeed={ApplyModelSpeed}
									onthinking={ApplyModelThinking}
									{permission_mode}
									permission_options={previewed_permissions?.options ?? []}
									permission_default={previewed_permissions?.default}
									policy={displayed_policy ?? policy}
									{selected_model_id}
									{speed_option_id}
									{thinking_level}
									onvariant={ApplyModelVariant}
									variant_options={previewed_variants}
								/>
							</div>
						</div>
					{/if}
				</div>
				</Tabs>
			</ShaderGlassSurface>
		</PopoverContent>
	</Popover>
</div>
</TooltipProvider>
