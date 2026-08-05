<script lang="ts" effect>
	import Selector from "@tabler/icons-svelte/icons/selector";
	import Tool from "@tabler/icons-svelte/icons/tool";
	import { Effect, Stream } from "effect";
	import { untrack } from "svelte";
	import {
		SessionPolicyPermission,
		type RuntimeCatalog,
		type ThreadSessionPolicy,
	} from "@artisan/protocol";
	import { ArtisanClient } from "@artisan/transport/client";
	import { BannerService } from "$lib/banner/service";
	import { EngineMarkClass, EngineMarkFor, ProviderMarkFor } from "$lib/engine/presentation";
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
	import ShaderGlassSurface from "../shader-glass-surface.sv";
	import EngineSection from "./engine-section.sv";
	import ModelList from "./model-list.sv";
	import {
		MakeModelPolicyController,
		type PolicyFlushResult,
	} from "./policy-controller";
	import PolicyControls from "./policy-controls.sv";
	import {
		ModelsFromCatalog,
		OrderModels,
		PermissionsForModel,
		permission_for_harness,
		permission_policy_for_harness,
		permission_reconciliation_for_harness,
		policy_fields_for_permission,
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
		engine_locked = false,
		onpolicychange,
		policy,
		runtime_catalog,
	}: {
		disabled?: boolean;
		/** Prevents a provider change while the current run is still in flight. */
		engine_locked?: boolean;
		onpolicychange?: (
			policy: ThreadSessionPolicy,
		) => Effect.Effect<ThreadSessionPolicy, { readonly message: string }>;
		policy?: ThreadSessionPolicy;
		runtime_catalog: RuntimeCatalog;
	} = $props();

	const client = yield* ArtisanClient;
	const banner = yield* BannerService;
	const defaults_controller = yield* SessionDefaultsController;
	let defaults_state = $state.raw<SessionDefaultsState | undefined>(undefined);
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
	let permission_mode = $state("supervised");
	let selected_model_id = $state(
		OfflineRuntimeCatalog.default_model_id ?? initial_models[0]?.id ?? "",
	);
	/**
	 * Forge owns the starred set, so every client opens the picker to the same
	 * order. It is read as its own statement rather than an awaited binding: a
	 * favorite is a nicety, and the picker must not wait on it to render.
	 */
	let favorite_ids = $state.raw<ReadonlyArray<string>>([]);
	const favorites_available = $derived(
		defaults_state?.available ?? !IsOfflineRuntimeCatalog(effective_catalog),
	);
	const policy_controller = yield* MakeModelPolicyController;
	let displayed_policy = $state.raw<ThreadSessionPolicy | undefined>(undefined);
	const RefreshFavorites = Effect.gen(function* () {
		const snapshot = yield* defaults_controller.Refresh;
		defaults_state = snapshot;
		favorite_ids = snapshot.favorite_ids;
		return snapshot;
	});

	const PersistFavorite = (request: ModelFavoriteRequest) =>
		Effect.gen(function* () {
			const snapshot = yield* defaults_controller.SetFavorite(
				request.model_id,
				request.favorite,
			);
			favorite_ids = snapshot.favorite_ids;
		}).pipe(
			Effect.catch((error) =>
				Effect.gen(function* () {
					yield* banner.error("Could not update model favorite", {
						description: error.message,
					});
					const snapshot = yield* RefreshFavorites.pipe(
						Effect.catch(() =>
							Effect.gen(function* () {
								return yield* defaults_controller.Current;
							}),
						),
					);
					favorite_ids = snapshot.favorite_ids;
				}),
			),
		);

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

	/**
	 * The replayed ready state hydrates once at mount; a real reconnect refreshes
	 * once more without clearing the last known set while Forge is unavailable.
	 */
	yield* client.ConnectionChanges.pipe(
		Stream.changesWith((left, right) => left.phase === right.phase),
		Stream.filter((state) => state.phase === "ready"),
		Stream.runForEach(() =>
			Effect.gen(function* () {
				yield* RefreshFavorites;
			}),
		),
		Effect.forkScoped,
	);
	yield* RefreshFavorites.pipe(
		Effect.catch(() =>
			Effect.gen(function* () {
			}),
		),
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
	 * across every model and engine; effort and context belong to the model that
	 * was configured, because their option sets differ per model. The write is
	 * deliberately secondary to the thread's authoritative policy: failure is
	 * reported, but it cannot roll back a thread policy Forge already accepted.
	 */
	const RememberDefaults = (next: ThreadSessionPolicy) =>
		Effect.gen(function* () {
			yield* defaults_controller.RememberPolicyDefaults(next).pipe(
				Effect.catch((error) =>
					Effect.gen(function* () {
						yield* banner.error("Could not save model defaults", {
							description: error.message,
						});
					}),
				),
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
			Effect.catchTag("ModelPolicyMutationError", (error) =>
				Effect.gen(function* () {
					yield* banner.error("Could not update thread policy", {
						description: error.message,
					});
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
		return OrderModels(models, active_engine, favorite_ids);
	});
	const selected_model = $derived(models.find((model) => model.id === selected_model_id) ?? models[0]);
	/**
	 * The speed only earns a word when it is not the model's own default: every
	 * model would otherwise trail a "Standard" that says nothing.
	 */
	const trigger_speed_label = $derived.by(() => {
		const speeds = selected_model?.definition.capabilities.speed_options ?? [];
		const selected = speeds.find((option) => option.id === speed_option_id);
		if (selected === undefined || selected.default) return undefined;
		return selected.label;
	});
	const selected_thinking_level = $derived(
		selected_model?.definition.capabilities.thinking.availability === "supported"
			? thinking_level
			: undefined,
	);
	const selected_engine = $derived(
		engines.find((engine) => engine.id === selected_model?.engine) ??
			engines[0] ?? { id: "codex", icon: Tool, monochrome: true, name: "Unavailable" },
	);
	const selected_harness = $derived(
		model_manifest.harnesses.find((harness) => harness.id === selected_model?.engine),
	);
	const selected_permission_options = $derived(selected_harness?.permissions.options ?? []);
	const previewed_model = $derived(
		models.find((model) => model.id === previewed_model_id) ?? selected_model,
	);
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
	const AdoptModel = (model: ModelChoice, context_suffix = "") =>
		Effect.gen(function* () {
			if (model.definition.disabled !== undefined) return false;
			if (engine_locked && model.engine !== selected_model?.engine) return false;

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
			const { context_window: _reset, ...rest } = base_policy;
			yield* ReplacePolicy({
				...rest,
				...policy_fields_for_permission(target_permission),
				...(context_suffix === "" ? {} : { context_window: context_suffix }),
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

	const ApplyModelContext = (model: ModelChoice, option: ContextWindowChoice) =>
		Effect.gen(function* () {
			if (model.id !== selected_model_id) {
				if (yield* AdoptModel(model, option.native_suffix)) yield* FlushPolicy;
				return;
			}
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
			if (selected_model?.id !== model.id && !(yield* AdoptModel(model))) return;
			thinking_level = level;
			yield* PatchPolicy({ reasoning_effort: PolicyEffortFromThinking(level) });
			yield* FlushPolicy;
		});

	const ApplyModelSpeed = (model: ModelChoice, option: SpeedOption) =>
		Effect.gen(function* () {
			if (selected_model?.id !== model.id && !(yield* AdoptModel(model))) return;
			if (option.disabled !== undefined) return;
			speed_option_id = option.id;
			yield* PatchPolicy({ service_tier: option.native_value });
			yield* FlushPolicy;
		});

	const ApplyModelPermission = (model: ModelChoice, option: PermissionOption) =>
		Effect.gen(function* () {
			if (selected_model?.id !== model.id && !(yield* AdoptModel(model))) return;
			permission_mode = option.id;
			yield* PatchPolicy(policy_fields_for_permission(option));
			yield* FlushPolicy;
		});

	const SyncAuthoritativePolicy = (next: ThreadSessionPolicy) =>
		Effect.gen(function* () {
			displayed_policy = yield* policy_controller.SetAuthoritative(next);
			const current = displayed_policy ?? next;
			const model = models.find(
				(candidate) =>
					candidate.definition.native_model_id === current.model ||
					(current.model === undefined && candidate.id === effective_catalog.default_model_id),
			);
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
		const engine = selected_model?.engine ?? "codex";
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
							class="model-trigger group/model-trigger flex h-8 shrink-0 items-center gap-2 rounded-[calc(var(--radius-3xl)-1rem)] bg-transparent px-2 text-left text-foreground outline-none transition-colors focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:ring-inset disabled:pointer-events-none"
						>
							{@const trigger_mark = ProviderMarkFor(selected_model?.definition.provider)}
							{@const TriggerMark = trigger_mark.icon}
							<span class="flex min-w-0 items-center gap-2">
								<TriggerMark class={EngineMarkClass(trigger_mark, "size-4")} />
								<span class="flex min-w-0 items-center gap-1 whitespace-nowrap text-sm">
									<span class="text-foreground">{selected_model?.name ?? "No models"}</span>
									{#if selected_thinking_level !== undefined}
										<span class="text-muted-foreground">
											{thinking_level_labels[selected_thinking_level]}
										</span>
									{/if}
									{#if trigger_speed_label !== undefined}
										<span class="text-muted-foreground">{trigger_speed_label}</span>
									{/if}
								</span>
							</span>
							<Selector class="model-trigger-chevron pointer-events-none size-3.5 shrink-0" />
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
			class="t-dropdown w-[min(30rem,calc(100vw-2rem))] rounded-3xl animate-none!"
		>
			<ShaderGlassSurface
				strength="strong"
				class="t-resize t-resize-auto w-full rounded-3xl"
			>
				<Tabs bind:value={active_engine} class="min-h-0 gap-2 p-2">
				<EngineSection
					{active_engine}
					{disabled}
					{disabled_reason}
					{engine_locked}
					{engines}
					{selected_engine}
				/>
				<div class="flex min-w-0 gap-2">
					<div class="model-scroll docs-scroll-fade h-48 min-w-0 grow overflow-y-auto rounded-xl">
						<ModelList
							{disabled}
							{favorite_ids}
							{favorites_available}
							models={active_models}
							onfavorite={RequestFavorite}
							onpreview={PreviewModel}
							onselect={SelectModel}
							{selected_model_id}
						/>
					</div>
					{#if previewed_model !== undefined}
						<div class="h-48 w-56 shrink-0">
							<div class="flex h-full flex-col justify-between gap-2 overflow-y-auto p-2.5">
								<div class="flex min-w-0 flex-col gap-1">
									<span class="truncate text-sm font-semibold text-foreground">{previewed_model.name}</span>
									{#if previewed_model.definition.description !== undefined}
										<span class="text-pretty text-xs text-muted-foreground">
											{previewed_model.definition.description}
										</span>
									{/if}
								</div>
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

<style src="./model-selector.css"></style>
