<script lang="ts" effect>
	import Selector from "@tabler/icons-svelte/icons/selector";
	import Tool from "@tabler/icons-svelte/icons/tool";
	import { Effect, Queue } from "effect";
	import {
		SessionPolicyPermission,
		type ThreadSessionPolicy,
	} from "@artisan/protocol";
	import { ArtisanClient } from "@artisan/transport/client";
	import { EngineMarkFor } from "$lib/engine/presentation";
	import {
		IsOfflineRuntimeCatalog,
		WithOfflineRuntimeCatalog,
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
	import CompactionControl from "./compaction-control.sv";
	import EngineSection from "./engine-section.sv";
	import ModelList from "./model-list.sv";
	import PolicyControls from "./policy-controls.sv";
	import {
		ModelsFromCatalog,
		OrderModels,
		PermissionsForModel,
		type ContextWindowChoice,
		type EngineChoice,
		type HarnessId,
		type ModelChoice,
		type PermissionOption,
		type SpeedOption,
		type ThinkingLevel,
	} from "$lib/engine/model-selection";

	type ModelFavoriteRequest = { readonly favorite: boolean; readonly model_id: string };

	let {
		disabled = false,
		engine_locked = false,
		onpolicychange,
		policy,
	}: {
		disabled?: boolean;
		/** Prevents a provider change while the current run is still in flight. */
		engine_locked?: boolean;
		onpolicychange?: (policy: ThreadSessionPolicy) => void;
		policy?: ThreadSessionPolicy;
	} = $props();

	const client = yield* ArtisanClient;
	const runtime_catalog = yield* WithOfflineRuntimeCatalog(client.GetRuntimeCatalog);
	const model_manifest = runtime_catalog.manifest;
	const engines: ReadonlyArray<EngineChoice> = model_manifest.harnesses.map((harness) => ({
		id: harness.id,
		name: harness.label,
		...EngineMarkFor(harness.id),
	}));

	const models = ModelsFromCatalog(runtime_catalog);
	let open = $state(false);
	let previewed_model_id = $state<string | undefined>(undefined);
	let thinking_level = $state<ThinkingLevel>("medium");
	let speed_option_id = $state("standard");
	let active_engine = $state<HarnessId>(models[0]?.engine ?? "codex");
	let permission_mode = $state("supervised");
	let selected_model_id = $state(runtime_catalog.default_model_id ?? models[0]?.id ?? "");
	let picker_surface = $state<HTMLElement | null>(null);
	/**
	 * Forge owns the starred set, so every client opens the picker to the same
	 * order. It is read as its own statement rather than an awaited binding: a
	 * favorite is a nicety, and the picker must not wait on it to render.
	 */
	let favorite_ids = $state.raw<ReadonlyArray<string>>([]);
	const favorites_available = !IsOfflineRuntimeCatalog(runtime_catalog);

	const favorite_requests = yield* Queue.unbounded<ModelFavoriteRequest>();
	const defaults_requests = yield* Queue.unbounded<Parameters<typeof client.UpdateSessionDefaults>[0]>();

	const RequestFavorite = (model_id: string, favorite: boolean) => {
		if (disabled || !favorites_available) return;
		/**
		 * The star fills under the pointer and Forge confirms afterwards. A
		 * failed write puts the previous set back, so the picker never keeps a
		 * star that was not durably recorded.
		 */
		favorite_ids = favorite
			? [...favorite_ids.filter((id) => id !== model_id), model_id]
			: favorite_ids.filter((id) => id !== model_id);
		Queue.offerUnsafe(favorite_requests, { favorite, model_id });
	};

	const HandleFavoriteRequest = (request: ModelFavoriteRequest) =>
		client.UpdateModelFavorite(request).pipe(
			Effect.andThen(client.GetModelFavorites),
			Effect.flatMap((snapshot) =>
				Effect.sync(() => {
					favorite_ids = snapshot.model_ids;
				}),
			),
			Effect.catch(() =>
				client.GetModelFavorites.pipe(
					Effect.flatMap((snapshot) =>
						Effect.sync(() => {
							favorite_ids = snapshot.model_ids;
						}),
					),
					Effect.ignore,
				),
			),
		);

	yield* Queue.take(favorite_requests).pipe(
		Effect.flatMap(HandleFavoriteRequest),
		Effect.forever,
		Effect.forkScoped,
	);
	yield* Queue.take(defaults_requests).pipe(
		Effect.flatMap((request) => client.UpdateSessionDefaults(request)),
		Effect.ignore,
		Effect.forever,
		Effect.forkScoped,
	);

	yield* client.GetModelFavorites.pipe(
		Effect.flatMap((snapshot) =>
			Effect.sync(() => {
				favorite_ids = snapshot.model_ids;
			}),
		),
		Effect.ignore,
	);

	/**
	 * Forge-owned choice of which catalog model writes handoff compaction
	 * summaries when a thread changes engine or model. Absent means each thread
	 * compacts with its own current model.
	 */
	let compaction_model_id = $state<string | undefined>(undefined);
	const compaction_available = !IsOfflineRuntimeCatalog(runtime_catalog);
	const thread_model_compaction_value = "__thread_model__";
	const compaction_models = models.filter((model) => model.definition.disabled === undefined);
	const compaction_model = $derived(
		compaction_models.find((model) => model.id === compaction_model_id),
	);

	yield* client.GetSessionDefaults.pipe(
		Effect.flatMap((defaults) =>
			Effect.sync(() => {
				compaction_model_id = defaults.compaction_model_id;
			}),
		),
		Effect.ignore,
	);

	/** Fire-and-forget like every other saved preference; the pick never waits. */
	const select_compaction_model = (value: string) => {
		const next = value === thread_model_compaction_value ? undefined : value;
		compaction_model_id = next;
		Queue.offerUnsafe(defaults_requests, { compaction_model_id: next ?? null });
	};

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
	 * fire-and-forget: a preference that fails to save must never block the pick
	 * the user just made.
	 */
	const RememberDefaults = (next: ThreadSessionPolicy) => {
		const model_id = models.find(
			(candidate) => candidate.definition.native_model_id === next.model,
		)?.id;

		Queue.offerUnsafe(defaults_requests, {
					...(next.model === undefined ? {} : { last_model_id: next.model }),
					...(model_id === undefined
						? {}
						: {
								model: {
									...(next.context_window === undefined
										? {}
										: { context_window: next.context_window }),
									model_id,
									reasoning_effort: next.reasoning_effort,
								},
							}),
					permission: SessionPolicyPermission(next),
				});
	};

	const PatchPolicy = (patch: Partial<ThreadSessionPolicy>) => {
		if (disabled || policy === undefined || onpolicychange === undefined) return;
		const next = { ...policy, ...patch };
		RememberDefaults(next);
		onpolicychange(next);
	};

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
			: PermissionsForModel(runtime_catalog, previewed_model),
	);
	/** The only way the whole selector disables: the thread session is not connected. */
	const disabled_reason = $derived(
		disabled ? "Unavailable until the thread's session is connected" : undefined,
	);

	/** Makes the model current without closing the popover; false when barred. */
	const adopt_model = (model: ModelChoice, context_suffix = ""): boolean => {
		if (model.definition.disabled !== undefined) {
			return false;
		}
		if (engine_locked && model.engine !== selected_model?.engine) {
			return false;
		}
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
		/**
		 * Permission options are harness-scoped, so a move to another engine
		 * keeps the current option only when that engine publishes it and
		 * otherwise falls back to the engine's own default.
		 */
		const target_permissions = model_manifest.harnesses.find(
			(harness) => harness.id === model.engine,
		)?.permissions;
		const target_permission =
			target_permissions?.options.find((option) => option.id === permission_mode) ??
			target_permissions?.options.find((option) => option.id === target_permissions.default) ??
			target_permissions?.options[0];
		if (target_permission !== undefined) permission_mode = target_permission.id;
		if (!disabled && policy !== undefined && onpolicychange !== undefined) {
			/** A different model invalidates the previous context-window suffix. */
			const { context_window: _reset, ...rest } = policy;
			const next: ThreadSessionPolicy = {
				...rest,
				...(target_permission === undefined
					? {}
					: {
							permission: target_permission.id,
							permission_mode:
								target_permission.approval_behavior === "none" ? "never" : "on_request",
							sandbox_mode:
								target_permission.edit_scope === "none" ? "read_only" : "workspace_write",
						}),
				...(context_suffix === "" ? {} : { context_window: context_suffix }),
				engine_id: model.engine,
				model: model.definition.native_model_id,
				reasoning_effort:
					model.definition.capabilities.thinking.availability === "supported"
						? PolicyEffortFromThinking(model.definition.capabilities.thinking.default)
						: policy.reasoning_effort,
				service_tier: default_speed?.native_value ?? "standard",
			};
			RememberDefaults(next);
			onpolicychange(next);
		}
		return true;
	};

	const apply_model_context = (model: ModelChoice, option: ContextWindowChoice) => {
		if (model.id !== selected_model_id) {
			adopt_model(model, option.native_suffix);
			return;
		}
		if (disabled || policy === undefined || onpolicychange === undefined) return;
		const { context_window: _previous, ...rest } = policy;
		const next: ThreadSessionPolicy =
			option.native_suffix === ""
				? rest
				: { ...rest, context_window: option.native_suffix };
		RememberDefaults(next);
		onpolicychange(next);
	};

	const select_model = (model: ModelChoice) => {
		if (adopt_model(model)) open = false;
	};

	const select_thinking_level = (level: ThinkingLevel) => {
		thinking_level = level;
		PatchPolicy({ reasoning_effort: PolicyEffortFromThinking(level) });
	};

	const select_speed = (option: SpeedOption) => {
		if (option.disabled !== undefined) {
			return;
		}
		speed_option_id = option.id;
		PatchPolicy({ service_tier: option.native_value });
	};

	/** An inline chip pick adopts its model first, then applies the option. */
	const apply_model_thinking = (model: ModelChoice, level: ThinkingLevel) => {
		if (selected_model?.id !== model.id && !adopt_model(model)) return;
		select_thinking_level(level);
	};

	const apply_model_speed = (model: ModelChoice, option: SpeedOption) => {
		if (selected_model?.id !== model.id && !adopt_model(model)) return;
		select_speed(option);
	};

	/**
	 * The option id is the durable choice; the two coarse axes travel with it
	 * for sandbox and tool gating and are derived from the catalog rather than
	 * from the id, so options past the neutral three survive a round trip.
	 */
	const select_permission = (option: PermissionOption) => {
		permission_mode = option.id;
		PatchPolicy({
			permission: option.id,
			permission_mode: option.approval_behavior === "none" ? "never" : "on_request",
			sandbox_mode: option.edit_scope === "none" ? "read_only" : "workspace_write",
		});
	};

	/**
	 * Permission belongs to the harness, not the model, but the pane's control
	 * acts on the previewed model like every other one there: picking an option
	 * for a model that is not current adopts that model first.
	 */
	const apply_model_permission = (model: ModelChoice, option: PermissionOption) => {
		if (selected_model?.id !== model.id && !adopt_model(model)) return;
		select_permission(option);
	};

	$effect(() => {
		if (policy === undefined) return;
		const model = models.find(
			(candidate) =>
				candidate.definition.native_model_id === policy.model ||
				(policy.model === undefined && candidate.id === runtime_catalog.default_model_id),
		);
		if (model !== undefined) selected_model_id = model.id;
		if (model !== undefined) active_engine = model.engine;
		thinking_level = ThinkingLevelFromPolicy(policy.reasoning_effort);
		permission_mode = PermissionModeFromPolicy(policy);
		const speed_option =
			model?.definition.capabilities.speed_options.find(
				(option) => option.native_value === policy.service_tier,
			) ?? model?.definition.capabilities.speed_options.find((option) => option.default);
		speed_option_id = speed_option?.id ?? "standard";
	});

	/** A fresh open previews the selected model until a row is hovered. */
	$effect(() => {
		if (!open) previewed_model_id = undefined;
	});

	$effect(() => {
		const options = selected_permission_options;
		if (options.length > 0 && !options.some((option) => option.id === permission_mode)) {
			permission_mode = selected_harness?.permissions.default ?? options[0]?.id ?? "";
		}
	});
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
							class="flex h-6 shrink-0 items-center gap-2 rounded-[calc(var(--radius-3xl)-1rem)] bg-transparent px-2 text-left text-foreground outline-none transition-colors focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:ring-inset disabled:pointer-events-none"
						>
							{@const SelectedIcon = selected_engine.icon}
							<SelectedIcon
								class={selected_engine.monochrome
									? "size-4 shrink-0 dark:invert"
									: "size-4 shrink-0"}
							/>
							<span class="whitespace-nowrap text-sm text-foreground">
								{selected_model?.name ?? "No models"}
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

		<PopoverContent
			bind:ref={picker_surface}
			variant="bare"
			align="end"
			side="top"
			sideOffset={8}
			class="w-[min(30rem,calc(100vw-2rem))] rounded-3xl"
			onOpenAutoFocus={(event) => {
				/**
				 * The default lands focus on the first engine tab, which is wrapped
				 * in a tooltip trigger — so opening the picker announced whichever
				 * engine happens to be first, locked or not. The panel itself takes
				 * focus instead; Tab still walks into the tabs from there.
				 */
				event.preventDefault();
				picker_surface?.focus();
			}}
		>
			<ShaderGlassSurface strength="strong" class="w-full rounded-3xl">
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
							onpreview={(model_id) => {
								previewed_model_id = model_id;
							}}
							onselect={select_model}
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
									oncontext={apply_model_context}
									onpermission={apply_model_permission}
									onspeed={apply_model_speed}
									onthinking={apply_model_thinking}
									{permission_mode}
									permission_options={previewed_permissions?.options ?? []}
									permission_default={previewed_permissions?.default}
									{policy}
									{selected_model_id}
									{speed_option_id}
									{thinking_level}
								/>
							</div>
						</div>
					{/if}
				</div>
				{#if compaction_available}
					<CompactionControl
						{disabled}
						model={compaction_model}
						models={compaction_models}
						onselect={select_compaction_model}
						thread_model_value={thread_model_compaction_value}
					/>
				{/if}
				</Tabs>
			</ShaderGlassSurface>
		</PopoverContent>
	</Popover>
</div>
</TooltipProvider>

<style src="./model-selector.css"></style>
