<script lang="ts" effect>
	import Selector from "@tabler/icons-svelte/icons/selector";
	import BoltFilled from "@tabler/icons-svelte/icons/bolt-filled";
	import Brain from "@tabler/icons-svelte/icons/brain";
	import Check from "@tabler/icons-svelte/icons/check";
	import Lock from "@tabler/icons-svelte/icons/lock";
	import Tool from "@tabler/icons-svelte/icons/tool";
	import { SvglOpenAILogo } from "@selemondev/svgl-svelte";
	import type { Component } from "svelte";
	import type { RuntimeCatalog, ThreadSessionPolicy } from "@artisan/protocol";
	import { ArtisanClient } from "@artisan/transport/client";
	import { EngineMarkFor } from "$lib/engine/presentation";
	import { remember_last_model } from "$lib/root/last-model";
	import send_gradient from "$lib/assets/composer/send-gradient.svg";

	import { Button } from "$lib/components/ui/button";
	import { Popover, PopoverContent, PopoverTrigger } from "$lib/components/ui/popover";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import { Tabs, TabsList, TabsTrigger } from "$lib/components/ui/tabs";
	import {
		Tooltip,
		TooltipContent,
		TooltipProvider,
		TooltipTrigger,
	} from "$lib/components/ui/tooltip";
	import DropdownHoverSurface from "./dropdown-hover-surface.sv";
	import ShaderGlassSurface from "./shader-glass-surface.sv";

	type ModelDefinition = RuntimeCatalog["manifest"]["models"][number];
	type HarnessId = ModelDefinition["harness"];
	type PermissionOption =
		RuntimeCatalog["manifest"]["harnesses"][number]["permissions"]["options"][number];
	type SpeedOption = ModelDefinition["capabilities"]["speed_options"][number];
	type ThinkingLevel = Exclude<
		ModelDefinition["capabilities"]["thinking"],
		{ readonly availability: "native" | "unavailable" }
	>["options"][number]["id"];

	type Engine = {
		id: HarnessId;
		name: string;
		icon: Component;
		monochrome: boolean;
	};

	type ModelChoice = {
		definition: ModelDefinition;
		id: string;
		engine: HarnessId;
		name: string;
		lab: string;
	};

	type ComposerControl = "model" | "speed" | "thinking" | "permission";

	let {
		disabled = false,
		engine_locked = false,
		onpolicychange,
		policy,
	}: {
		disabled?: boolean;
		/** A session that has produced conversation cannot move to another engine. */
		engine_locked?: boolean;
		onpolicychange?: (policy: ThreadSessionPolicy) => void;
		policy?: ThreadSessionPolicy;
	} = $props();

	const client = yield* ArtisanClient;
	const runtime_catalog = yield* client.GetRuntimeCatalog;
	const model_manifest = runtime_catalog.manifest;
	const engines: ReadonlyArray<Engine> = model_manifest.harnesses.map((harness) => ({
		id: harness.id,
		name: harness.label,
		...EngineMarkFor(harness.id),
	}));
	const thinking_level_labels: Readonly<Record<ThinkingLevel, string>> = {
		high: "High",
		light: "Light",
		max: "Max",
		medium: "Medium",
		xhigh: "Extra High",
	};

	const harness_labels = new Map(model_manifest.harnesses.map((harness) => [harness.id, harness.label]));
	const models: ReadonlyArray<ModelChoice> = model_manifest.models.map((model) => ({
		definition: model,
		engine: model.harness,
		id: model.id,
		lab: harness_labels.get(model.harness) ?? model.harness,
		name: model.name,
	}));
	let open = $state(false);
	let permission_open = $state(false);
	let speed_open = $state(false);
	let thinking_open = $state(false);
	let thinking_level = $state<ThinkingLevel>("medium");
	let speed_option_id = $state("standard");
	let active_engine = $state<HarnessId>(models[0]?.engine ?? "codex");
	/** Tracks the harness-neutral permission option id, never a native value. */
	let permission_mode = $state("supervised");
	let selected_model_id = $state(runtime_catalog.default_model_id ?? models[0]?.id ?? "");
	let engine_surface = $state<HTMLElement | null>(null);
	let engine_indicator_animated = $state(false);
	let engine_indicator_left = $state(0);
	let engine_indicator_visible = $state(false);
	let engine_indicator_width = $state(0);

	const ThinkingLevelFromPolicy = (
		effort: ThreadSessionPolicy["reasoning_effort"],
	): ThinkingLevel => (effort === "low" ? "light" : effort);
	const PolicyEffortFromThinking = (
		level: ThinkingLevel,
	): ThreadSessionPolicy["reasoning_effort"] =>
		level === "light" ? "low" : level;
	const PermissionModeFromPolicy = (value: ThreadSessionPolicy) =>
		value.sandbox_mode === "read_only"
			? "restricted"
			: value.permission_mode === "never"
				? "autonomous"
				: "supervised";
	const PatchPolicy = (patch: Partial<ThreadSessionPolicy>) => {
		if (disabled || policy === undefined || onpolicychange === undefined) return;
		const next = { ...policy, ...patch };
		remember_last_model(next);
		onpolicychange(next);
	};

	const active_models = $derived(
		models.filter((model) => model.engine === active_engine),
	);
	const selected_model = $derived(models.find((model) => model.id === selected_model_id) ?? models[0]);
	const selected_speed_options = $derived(
		selected_model?.definition.capabilities.speed_options ?? [],
	);
	const selected_speed_option = $derived(
		selected_speed_options.find(
			(option) => option.id === speed_option_id && option.disabled === undefined,
		) ??
			selected_speed_options.find(
				(option) => option.default && option.disabled === undefined,
			) ??
			selected_speed_options.find((option) => option.disabled === undefined),
	);
	const selected_thinking = $derived(
		selected_model?.definition.capabilities.thinking ?? { availability: "unavailable" as const },
	);
	const selected_thinking_native_value = $derived(
		selected_thinking.availability === "supported"
			? selected_thinking.options.find((option) => option.id === thinking_level)?.native_value
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
	const selected_permission = $derived(
		selected_permission_options.find((option) => option.id === permission_mode) ??
			selected_permission_options[0],
	);
	const composer_controls: ReadonlyArray<ComposerControl> = ["model", "thinking", "speed"];
	/** The only way the whole selector disables: the thread session is not connected. */
	const disabled_reason = $derived(
		disabled ? "Unavailable until the thread's session is connected" : undefined,
	);
	const enabled_speed_options = $derived(
		selected_speed_options.filter((option) => option.disabled === undefined),
	);
	const default_speed_option = $derived(
		enabled_speed_options.find((option) => option.default) ?? enabled_speed_options[0],
	);
	const speed_engaged = $derived(
		selected_speed_option !== undefined &&
			selected_speed_option.id !== default_speed_option?.id,
	);

	const select_model = (model: ModelChoice) => {
		if (model.definition.disabled !== undefined) {
			return;
		}
		if (engine_locked && model.engine !== selected_model?.engine) {
			return;
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
		PatchPolicy({
			engine_id: model.engine,
			model: model.definition.native_model_id,
			reasoning_effort:
				model.definition.capabilities.thinking.availability === "supported"
					? PolicyEffortFromThinking(model.definition.capabilities.thinking.default)
					: (policy?.reasoning_effort ?? "medium"),
			service_tier: default_speed?.native_value ?? "standard",
		});
		open = false;
	};

	const select_thinking_level = (level: ThinkingLevel) => {
		thinking_level = level;
		PatchPolicy({ reasoning_effort: PolicyEffortFromThinking(level) });
		thinking_open = false;
	};

	const select_speed = (option: SpeedOption) => {
		if (option.disabled !== undefined) {
			return;
		}
		speed_option_id = option.id;
		PatchPolicy({ service_tier: option.native_value });
	};

	const toggle_speed = () => {
		if (enabled_speed_options.length < 2) {
			return;
		}
		const index = enabled_speed_options.findIndex(
			(option) => option.id === selected_speed_option?.id,
		);
		const next = enabled_speed_options[(index + 1) % enabled_speed_options.length];
		if (next !== undefined) select_speed(next);
	};

	const select_permission = (option: PermissionOption) => {
		permission_mode = option.id;
		PatchPolicy(
			option.id === "restricted"
				? { permission_mode: "never", sandbox_mode: "read_only" }
				: option.id === "autonomous"
					? { permission_mode: "never", sandbox_mode: "workspace_write" }
					: { permission_mode: "on_request", sandbox_mode: "workspace_write" },
		);
		permission_open = false;
	};

	const position_engine_indicator = (animate: boolean) => {
		const active_tab = engine_surface?.querySelector<HTMLElement>(
			`[data-engine="${active_engine}"]`,
		);

		if (!active_tab || !engine_surface) {
			return;
		}

		const surface_rect = engine_surface.getBoundingClientRect();
		const tab_rect = active_tab.getBoundingClientRect();

		engine_indicator_animated = animate && engine_indicator_visible;
		engine_indicator_left = tab_rect.left - surface_rect.left;
		engine_indicator_visible = true;
		engine_indicator_width = tab_rect.width;
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

	$effect(() => {
		void active_engine;
		void engine_surface;

		const frame = requestAnimationFrame(() => position_engine_indicator(true));

		return () => cancelAnimationFrame(frame);
	});

	$effect(() => {
		const options = selected_permission_options;
		if (options.length > 0 && !options.some((option) => option.id === permission_mode)) {
			permission_mode = selected_harness?.permissions.default ?? options[0]?.id ?? "";
		}
	});
</script>

<svelte:window onresize={() => position_engine_indicator(false)} />

<TooltipProvider delayDuration={0}>
<ShaderGlassSurface
	use_rays={false}
	class="min-w-0 max-w-full rounded-[calc(var(--radius-3xl)-0.875rem)]"
>
<div
	class="no-scrollbar flex min-w-0 max-w-full items-center gap-1 overflow-x-auto p-0.5"
	style={`--composer-accent-image: url("${send_gradient}")`}
>
	{#each composer_controls as control (control)}
	{#if control === "model"}
	<Popover bind:open>
		<Tooltip>
			<TooltipTrigger>
				{#snippet child({ props: tooltip_props })}
					<span {...tooltip_props} class="flex min-w-0 has-[:disabled]:cursor-not-allowed">
						<PopoverTrigger
							aria-label="Select model"
							disabled={disabled}
							class="flex h-6 min-w-0 max-w-44 shrink-0 items-center gap-2 rounded-[calc(var(--radius-3xl)-1rem)] bg-transparent px-2 text-left text-foreground outline-none transition-colors focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:ring-inset disabled:pointer-events-none"
						>
							{@const SelectedIcon = selected_engine.icon}
							<SelectedIcon
								class={selected_engine.monochrome
									? "size-4 shrink-0 dark:invert"
									: "size-4 shrink-0"}
							/>
							<span class="truncate text-sm text-foreground">{selected_model?.name ?? "No models"}</span>
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
			variant="bare"
			align="end"
			side="top"
			sideOffset={8}
			class="w-[min(20rem,calc(100vw-2rem))] rounded-3xl"
		>
			<ShaderGlassSurface strength="strong" class="w-full rounded-3xl">
				<Tabs bind:value={active_engine} class="min-h-0 gap-2 p-2">
				<TabsList
					bind:ref={engine_surface}
					variant="line"
					aria-label="Coding engines"
					class="card relative h-auto! w-full justify-start overflow-x-auto rounded-lg! bg-linear-to-b from-surface-225 to-surface-200 p-1 dark:from-surface-800 dark:to-surface-925"
				>
					<div
						class="model-selector-engine-light"
						data-active={engine_indicator_visible}
						data-animate={engine_indicator_animated}
						aria-hidden="true"
						style={`--engine-light-x: ${engine_indicator_left}px; --engine-light-width: ${engine_indicator_width}px;`}
					></div>
					{#each engines as engine (engine.id)}
						{@const EngineIcon = engine.icon}
						{@const engine_disabled_reason =
							engine_locked && engine.id !== selected_engine.id
								? `${engine.name} — engine is locked for this session`
								: disabled_reason}
						<Tooltip>
							<TooltipTrigger>
								{#snippet child({ props: tooltip_props })}
									<span
										{...tooltip_props}
										class="flex flex-none has-[:disabled]:cursor-not-allowed"
									>
										<TabsTrigger
											value={engine.id}
											disabled={disabled ||
												(engine_locked && engine.id !== selected_engine.id)}
											data-engine={engine.id}
											aria-label={engine.name}
											class="relative z-1 size-8 flex-none px-0 text-foreground after:hidden hover:text-foreground data-active:border-transparent data-active:bg-transparent data-active:text-foreground dark:hover:text-foreground dark:data-active:border-transparent dark:data-active:bg-transparent"
										>
											<EngineIcon class={engine.monochrome ? "size-4 dark:invert" : "size-4"} />
										</TabsTrigger>
									</span>
								{/snippet}
							</TooltipTrigger>
							{#if engine_disabled_reason !== undefined}
								<TooltipContent>{engine_disabled_reason}</TooltipContent>
							{/if}
						</Tooltip>
					{/each}
				</TabsList>
			<ScrollArea class="h-48 rounded-xl">
				<DropdownHoverSurface
					class="[--docs-sidebar-hover-radius:calc(var(--radius-3xl)-0.5rem)]"
				>
					{#snippet children({ move_hover })}
						<table class="w-full border-separate border-spacing-y-0.5" aria-label="Available models">
							<tbody>
								{#each active_models as model (model.id)}
									{@const ModelIcon = engines.find((engine) => engine.id === model.engine)?.icon ?? SvglOpenAILogo}
									{@const is_monochrome = engines.find((engine) => engine.id === model.engine)?.monochrome ?? false}
									<tr>
										<td class="p-0">
											<button
												type="button"
												disabled={disabled || model.definition.disabled !== undefined}
												title={model.definition.disabled?.reason}
												class="flex w-full items-center gap-2 rounded-[calc(var(--radius-3xl)-0.5rem)] px-2.5 py-1.5 text-left focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
												aria-current={model.id === selected_model_id ? "true" : undefined}
												onfocus={move_hover}
												onpointerenter={move_hover}
												onclick={() => select_model(model)}
											>
												<ModelIcon
													class={is_monochrome
														? "size-5 shrink-0 dark:invert"
														: "size-5 shrink-0"}
												/>
												<span class="flex min-w-0 flex-col space-y-0">
													<span class="truncate text-sm font-semibold text-foreground">{model.name}</span>
													<span class="truncate text-xs text-muted-foreground">{model.lab}</span>
													{#if model.definition.disabled !== undefined}
														<span class="truncate text-xs text-muted-foreground">
															{model.definition.disabled.reason}
														</span>
													{/if}
												</span>
											</button>
										</td>
									</tr>
								{/each}
							</tbody>
						</table>
					{/snippet}
				</DropdownHoverSurface>
			</ScrollArea>
				</Tabs>
			</ShaderGlassSurface>
		</PopoverContent>
	</Popover>
	{/if}

	{#if control === "thinking" && selected_thinking.availability === "supported"}
		<Popover bind:open={thinking_open}>
			<Tooltip>
				<TooltipTrigger>
					{#snippet child({ props: tooltip_props })}
						<span {...tooltip_props} class="flex shrink-0 has-[:disabled]:cursor-not-allowed">
							<PopoverTrigger
								aria-label={`Thinking: ${thinking_level_labels[thinking_level]}`}
								disabled={disabled}
								data-native-value={selected_thinking_native_value}
								class="flex h-6 shrink-0 items-center gap-1 rounded-[calc(var(--radius-3xl)-1rem)] bg-transparent px-2 text-sm text-foreground outline-none transition-colors focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:ring-inset disabled:pointer-events-none"
							>
								<Brain class="size-4 shrink-0 text-muted-foreground" />
								<span>{thinking_level_labels[thinking_level]}</span>
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
				variant="bare"
				align="end"
				side="top"
				sideOffset={8}
				class="w-56 rounded-3xl"
			>
				<ShaderGlassSurface strength="strong" class="w-full rounded-3xl">
					<DropdownHoverSurface
						class="p-2 [--docs-sidebar-hover-radius:calc(var(--radius-3xl)-0.5rem)]"
					>
						{#snippet children({ move_hover })}
							<div class="flex flex-col gap-0.5">
								{#each selected_thinking.options as option (option.id)}
									<button
										type="button"
										disabled={disabled}
										class="flex w-full items-center justify-between gap-3 rounded-[calc(var(--radius-3xl)-0.5rem)] px-2.5 py-1.5 text-left focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none"
										aria-current={option.id === thinking_level ? "true" : undefined}
										onfocus={move_hover}
										onpointerenter={move_hover}
										onclick={() => select_thinking_level(option.id)}
									>
										<span class="text-sm font-semibold text-foreground">
											{thinking_level_labels[option.id]}
										</span>
										{#if option.id === thinking_level}
											<Check class="size-4 shrink-0 text-muted-foreground" />
										{/if}
									</button>
								{/each}
							</div>
						{/snippet}
					</DropdownHoverSurface>
				</ShaderGlassSurface>
			</PopoverContent>
		</Popover>
	{/if}

	{#if control === "speed"}
		{#if enabled_speed_options.length > 2}
			<Popover bind:open={speed_open}>
				<Tooltip>
					<TooltipTrigger>
						{#snippet child({ props: tooltip_props })}
							<span {...tooltip_props} class="flex shrink-0 has-[:disabled]:cursor-not-allowed">
								<PopoverTrigger disabled={disabled}>
									{#snippet child({ props: popover_props })}
										<Button
											{...popover_props}
											variant="ghost"
											size="icon-xs"
											disabled={disabled}
											aria-label={`Speed: ${selected_speed_option?.label ?? "Select"}`}
											class={`rounded-[calc(var(--radius-3xl)-1rem)] text-muted-foreground transition-colors ${speed_engaged ? "composer-accent-button inset-shadow text-white hover:text-white" : ""}`}
										>
											<BoltFilled />
										</Button>
									{/snippet}
								</PopoverTrigger>
							</span>
						{/snippet}
					</TooltipTrigger>
					{#if disabled_reason !== undefined}
						<TooltipContent>{disabled_reason}</TooltipContent>
					{/if}
				</Tooltip>
				<PopoverContent
					variant="bare"
					side="top"
					sideOffset={6}
					align="end"
					class="w-72 rounded-2xl"
				>
					<ShaderGlassSurface strength="strong" class="w-full rounded-2xl">
						<DropdownHoverSurface
							class="p-1.5 [--docs-sidebar-hover-radius:calc(var(--radius-2xl)-0.375rem)]"
						>
							{#snippet children({ move_hover })}
								<div class="flex flex-col gap-0.5">
									{#each selected_speed_options as option (option.id)}
										<button
											type="button"
											disabled={disabled || option.disabled !== undefined}
											title={option.disabled?.reason}
											class="flex w-full items-start justify-between gap-3 rounded-[calc(var(--radius-2xl)-0.375rem)] px-3 py-2 text-left focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-45"
											onfocus={move_hover}
											onpointerenter={move_hover}
											onclick={() => {
												select_speed(option);
												speed_open = false;
											}}
										>
											<span class="flex min-w-0 flex-col">
												<span class="text-sm text-foreground">{option.label}</span>
												<span class="text-sm text-muted-foreground">{option.description}</span>
												{#if option.disabled !== undefined}
													<span class="text-xs text-muted-foreground">
														{option.disabled.reason}
													</span>
												{/if}
											</span>
											{#if option.id === speed_option_id}
												<Check class="size-4 shrink-0 self-center text-muted-foreground" />
											{/if}
										</button>
									{/each}
								</div>
							{/snippet}
						</DropdownHoverSurface>
					</ShaderGlassSurface>
				</PopoverContent>
			</Popover>
		{:else}
			<Tooltip>
				<TooltipTrigger>
					{#snippet child({ props: tooltip_props })}
						<span {...tooltip_props} class="flex shrink-0 has-[:disabled]:cursor-not-allowed">
							<Button
								variant="ghost"
								size="icon-xs"
								aria-label={`Speed: ${selected_speed_option?.label ?? "Unavailable"}`}
								aria-pressed={speed_engaged}
								disabled={disabled || enabled_speed_options.length < 2}
								class={`rounded-[calc(var(--radius-3xl)-1rem)] text-muted-foreground transition-colors ${speed_engaged ? "composer-accent-button inset-shadow text-white hover:text-white" : ""}`}
								onclick={toggle_speed}
							>
								<BoltFilled />
							</Button>
						</span>
					{/snippet}
				</TooltipTrigger>
				{#if disabled || enabled_speed_options.length < 2}
					<TooltipContent>
						{disabled_reason ?? "This model has a single speed"}
					</TooltipContent>
				{/if}
			</Tooltip>
		{/if}
	{/if}

	{#if control === "permission"}
		<Popover bind:open={permission_open}>
			<PopoverTrigger
				aria-label="Permission"
				disabled={disabled}
				class="flex h-8 shrink-0 items-center gap-1.5 rounded-xl bg-transparent px-2 text-sm text-foreground outline-none transition-colors focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:ring-inset"
			>
				<Lock class="size-4 text-muted-foreground" />
				<span class="hidden sm:inline">{selected_permission?.label ?? "Permission"}</span>
				<Selector class="size-3.5 text-muted-foreground" />
			</PopoverTrigger>
			<PopoverContent
				variant="bare"
				side="top"
				sideOffset={6}
				align="end"
				class="w-72 rounded-2xl"
			>
				<ShaderGlassSurface strength="strong" class="w-full rounded-2xl">
					<DropdownHoverSurface
						class="p-1.5 [--docs-sidebar-hover-radius:calc(var(--radius-2xl)-0.375rem)]"
					>
						{#snippet children({ move_hover })}
							<div class="flex flex-col gap-0.5">
								{#each selected_permission_options as option (option.id)}
									<button
										type="button"
										disabled={disabled}
										class="flex w-full items-start justify-between gap-3 rounded-[calc(var(--radius-2xl)-0.375rem)] px-3 py-2 text-left focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none"
										onfocus={move_hover}
										onpointerenter={move_hover}
										onclick={() => select_permission(option)}
									>
										<span class="flex min-w-0 flex-col">
											<span class="text-sm text-foreground">{option.label}</span>
											<span class="text-sm text-muted-foreground">{option.description}</span>
										</span>
										{#if option.id === permission_mode}
											<Check class="size-4 shrink-0 self-center text-muted-foreground" />
										{/if}
									</button>
								{/each}
							</div>
						{/snippet}
					</DropdownHoverSurface>
				</ShaderGlassSurface>
			</PopoverContent>
		</Popover>
	{/if}

	{/each}
</div>
</ShaderGlassSurface>
</TooltipProvider>

<style>
	.model-selector-engine-light {
		position: absolute;
		top: 0;
		left: 0;
		z-index: 0;
		width: var(--engine-light-width, 0);
		height: 100%;
		pointer-events: none;
		opacity: 0;
		transform: translate3d(var(--engine-light-x, 0), 0, 0);
		will-change: transform, width;
	}

	.model-selector-engine-light::after {
		position: absolute;
		top: -0.125rem;
		left: 50%;
		width: 2rem;
		height: 1.5rem;
		content: "";
		background: linear-gradient(
			to bottom,
			oklch(from var(--foreground) l c h / 32%) 0%,
			oklch(from var(--foreground) l c h / 10%) 26%,
			oklch(from var(--foreground) l c h / 2%) 52%,
			transparent 74%
		);
		filter: blur(0.125rem);
		-webkit-mask-image: radial-gradient(
			ellipse 48% 70% at 50% 35%,
			#000 0%,
			rgb(0 0 0 / 50%) 42%,
			rgb(0 0 0 / 10%) 68%,
			transparent 88%
		);
		mask-image: radial-gradient(
			ellipse 48% 70% at 50% 35%,
			#000 0%,
			rgb(0 0 0 / 50%) 42%,
			rgb(0 0 0 / 10%) 68%,
			transparent 88%
		);
		transform: translateX(-50%);
	}

	.model-selector-engine-light[data-active="true"] {
		opacity: 1;
	}

	.model-selector-engine-light[data-animate="true"] {
		transition:
			transform var(--duration-fast) var(--ease-smooth-out),
			width var(--duration-fast) var(--ease-smooth-out);
	}

	@media (prefers-reduced-motion: reduce) {
		.model-selector-engine-light {
			transition: none !important;
		}
	}

	/**
	 * The send button's amplified inner shadow, applied while the gradient
	 * artwork is visible so the engaged chip gets the same carved rim.
	 */
	:global(.composer-accent-button.inset-shadow) {
		box-shadow:
			inset 0px 1px 1px -0.5px rgba(0, 0, 0, 0.3),
			inset 0px 3px 3px -1.5px rgba(0, 0, 0, 0.26),
			inset 0px 6px 6px -3px rgba(0, 0, 0, 0.22),
			inset 0 -0.5px rgba(255, 255, 255, 0.5),
			inset 0 0 0 0.5px oklch(from var(--highlight) l c h / 35%);
	}

	/**
	 * Engaged controls borrow the send button's treatment: the gradient artwork
	 * as an opaque background, a full-white glyph blended with overlay so the
	 * artwork lights it, and isolation to keep the blend inside the button.
	 */
	:global(.composer-accent-button) {
		background-image: var(--composer-accent-image);
		background-size: cover;
		background-position: center;
		isolation: isolate;
	}

	:global(.composer-accent-button svg) {
		mix-blend-mode: overlay;
	}
</style>
