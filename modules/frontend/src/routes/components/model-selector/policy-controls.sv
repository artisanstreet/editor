<script lang="ts" effect>
	import ArrowsHorizontal from "@tabler/icons-svelte/icons/arrows-horizontal";
	import BoltFilled from "@tabler/icons-svelte/icons/bolt-filled";
	import Brain from "@tabler/icons-svelte/icons/brain";
	import Lock from "@tabler/icons-svelte/icons/lock";
	import { Effect } from "effect";
	import type { ThreadSessionPolicy } from "@artisan/protocol";
	import { MakeFollowHighlight } from "$lib/components/dropdown-highlight";
	import { Select, SelectContent, SelectItem, SelectTrigger } from "$lib/components/ui/select";
	import { Tooltip, TooltipContent, TooltipTrigger } from "$lib/components/ui/tooltip";
	import {
		thinking_level_labels,
		type ContextWindowChoice,
		type ModelChoice,
		type PermissionOption,
		type SpeedOption,
		type ThinkingLevel,
	} from "$lib/engine/model-selection";
	import DropdownHoverSurface from "../dropdown-hover-surface.sv";
	import ShaderGlassSurface from "../shader-glass-surface.sv";

	const FollowHighlight = yield* MakeFollowHighlight;

	let {
		disabled,
		model,
		oncontext,
		onpermission,
		onspeed,
		onthinking,
		permission_mode,
		permission_options,
		permission_default,
		policy,
		selected_model_id,
		speed_option_id,
		thinking_level,
	}: {
		disabled: boolean;
		model: ModelChoice;
		oncontext: (model: ModelChoice, option: ContextWindowChoice) => Effect.Effect<void>;
		onpermission: (model: ModelChoice, option: PermissionOption) => Effect.Effect<void>;
		onspeed: (model: ModelChoice, option: SpeedOption) => Effect.Effect<void>;
		onthinking: (model: ModelChoice, level: ThinkingLevel) => Effect.Effect<void>;
		permission_mode: string;
		permission_options: ReadonlyArray<PermissionOption>;
		permission_default?: string;
		policy?: ThreadSessionPolicy;
		selected_model_id: string;
		speed_option_id: string;
		thinking_level: ThinkingLevel;
	} = $props();

	const thinking = $derived(model.definition.capabilities.thinking);
	const speeds = $derived(
		model.definition.capabilities.speed_options.filter((option) => option.disabled === undefined),
	);
	const context = $derived(model.definition.capabilities.context_window);
	const current_thinking = $derived(
		thinking.availability === "supported"
			? model.id === selected_model_id
				? thinking_level
				: thinking.default
			: undefined,
	);
	const current_speed = $derived(
		(model.id === selected_model_id
			? speeds.find((option) => option.id === speed_option_id)
			: undefined) ??
			speeds.find((option) => option.default) ??
			speeds[0],
	);
	const current_context = $derived(
		context === undefined
			? undefined
			: ((model.id === selected_model_id
					? context.options.find((option) =>
							policy?.context_window === undefined
								? option.native_suffix === ""
								: option.native_suffix === policy.context_window,
						)
					: undefined) ??
				context.options.find((option) => option.id === context.default) ??
				context.options[0]),
	);
	const current_permission = $derived(
		(model.id === selected_model_id
			? permission_options.find((option) => option.id === permission_mode)
			: undefined) ??
			permission_options.find((option) => option.id === permission_default) ??
			permission_options[0],
	);

	const SelectThinking = (value: string) =>
		Effect.gen(function* () {
			if (thinking.availability !== "supported") return;
			const level = thinking.options.find((option) => option.id === value)?.id;
			if (level !== undefined) yield* onthinking(model, level);
		});

	const SelectSpeed = (value: string) =>
		Effect.gen(function* () {
			const option = speeds.find((candidate) => candidate.id === value);
			if (option !== undefined) yield* onspeed(model, option);
		});

	const SelectContext = (value: string) =>
		Effect.gen(function* () {
			if (context === undefined) return;
			const option = context.options.find((candidate) => candidate.id === value);
			if (option !== undefined) yield* oncontext(model, option);
		});

	const SelectPermission = (value: string) =>
		Effect.gen(function* () {
			const option = permission_options.find((candidate) => candidate.id === value);
			if (option !== undefined) yield* onpermission(model, option);
		});
</script>

<div class="flex flex-col gap-1.5">
	{#if thinking.availability === "supported" && current_thinking !== undefined}
		<Select
			type="single"
			value={current_thinking}
			onValueChange={yield* SelectThinking(event)}
			{disabled}
		>
			<div class="card min-w-0 rounded-md bg-linear-to-b from-surface-225 to-surface-200 dark:from-surface-800 dark:to-surface-925">
				<SelectTrigger
					size="sm"
					class="h-6 w-full border-transparent bg-transparent px-2 text-xs shadow-none data-[size=sm]:h-6 dark:bg-transparent dark:hover:bg-transparent dark:hover:text-foreground"
				>
					<Brain class="size-3.5 shrink-0 text-muted-foreground" />
					<span class="truncate">{thinking_level_labels[current_thinking]}</span>
				</SelectTrigger>
			</div>
			<SelectContent class="rounded-2xl border-transparent bg-transparent p-0 shadow-none">
				<ShaderGlassSurface strength="strong" class="rounded-2xl p-1">
					<DropdownHoverSurface class="[--docs-sidebar-hover-radius:var(--radius-xl)]">
						{#snippet children({ move_hover })}
							{#each thinking.options as option (option.id)}
								<SelectItem
									value={option.id}
									class="focus:bg-transparent! data-highlighted:bg-transparent! data-highlighted:text-foreground!"
									{@attach FollowHighlight(move_hover)}
								>
									{thinking_level_labels[option.id]}
								</SelectItem>
							{/each}
						{/snippet}
					</DropdownHoverSurface>
				</ShaderGlassSurface>
			</SelectContent>
		</Select>
	{/if}

	{#if speeds.length > 1}
		<Select
			type="single"
			value={current_speed?.id}
			onValueChange={yield* SelectSpeed(event)}
			{disabled}
		>
			<div class="card min-w-0 rounded-md bg-linear-to-b from-surface-225 to-surface-200 dark:from-surface-800 dark:to-surface-925">
				<SelectTrigger
					size="sm"
					class="h-6 w-full border-transparent bg-transparent px-2 text-xs shadow-none data-[size=sm]:h-6 dark:bg-transparent dark:hover:bg-transparent dark:hover:text-foreground"
				>
					<BoltFilled class="size-3.5 shrink-0 text-muted-foreground" />
					<span class="truncate">{current_speed?.label ?? "Speed"}</span>
				</SelectTrigger>
			</div>
			<SelectContent class="rounded-2xl border-transparent bg-transparent p-0 shadow-none">
				<ShaderGlassSurface strength="strong" class="rounded-2xl p-1">
					<DropdownHoverSurface class="[--docs-sidebar-hover-radius:var(--radius-xl)]">
						{#snippet children({ move_hover })}
							{#each speeds as speed (speed.id)}
								<Tooltip>
									<TooltipTrigger>
										{#snippet child({ props: tooltip_props })}
											<span {...tooltip_props} class="flex">
												<SelectItem
													value={speed.id}
													class="w-full focus:bg-transparent! data-highlighted:bg-transparent! data-highlighted:text-foreground!"
													{@attach FollowHighlight(move_hover)}
												>
													{speed.label}
												</SelectItem>
											</span>
										{/snippet}
									</TooltipTrigger>
									<TooltipContent side="right" class="max-w-56">
										{speed.description}
									</TooltipContent>
								</Tooltip>
							{/each}
						{/snippet}
					</DropdownHoverSurface>
				</ShaderGlassSurface>
			</SelectContent>
		</Select>
	{/if}

	{#if context !== undefined && current_context !== undefined}
		<Select
			type="single"
			value={current_context.id}
			onValueChange={yield* SelectContext(event)}
			{disabled}
		>
			<div class="card min-w-0 rounded-md bg-linear-to-b from-surface-225 to-surface-200 dark:from-surface-800 dark:to-surface-925">
				<SelectTrigger
					size="sm"
					class="h-6 w-full border-transparent bg-transparent px-2 text-xs shadow-none data-[size=sm]:h-6 dark:bg-transparent dark:hover:bg-transparent dark:hover:text-foreground"
				>
					<ArrowsHorizontal class="size-3.5 shrink-0 text-muted-foreground" />
					<span class="truncate">{current_context.label}</span>
				</SelectTrigger>
			</div>
			<SelectContent class="rounded-2xl border-transparent bg-transparent p-0 shadow-none">
				<ShaderGlassSurface strength="strong" class="rounded-2xl p-1">
					<DropdownHoverSurface class="[--docs-sidebar-hover-radius:var(--radius-xl)]">
						{#snippet children({ move_hover })}
							{#each context.options as option (option.id)}
								<Tooltip>
									<TooltipTrigger>
										{#snippet child({ props: tooltip_props })}
											<span {...tooltip_props} class="flex">
												<SelectItem
													value={option.id}
													class="w-full focus:bg-transparent! data-highlighted:bg-transparent! data-highlighted:text-foreground!"
													{@attach FollowHighlight(move_hover)}
												>
													{option.label}
												</SelectItem>
											</span>
										{/snippet}
									</TooltipTrigger>
									<TooltipContent side="right" class="max-w-56">
										{option.description}
									</TooltipContent>
								</Tooltip>
							{/each}
						{/snippet}
					</DropdownHoverSurface>
				</ShaderGlassSurface>
			</SelectContent>
		</Select>
	{/if}

	{#if permission_options.length > 1 && current_permission !== undefined}
		<Select
			type="single"
			value={current_permission.id}
			onValueChange={yield* SelectPermission(event)}
			{disabled}
		>
			<div class="card min-w-0 rounded-md bg-linear-to-b from-surface-225 to-surface-200 dark:from-surface-800 dark:to-surface-925">
				<SelectTrigger
					size="sm"
					class="h-6 w-full border-transparent bg-transparent px-2 text-xs shadow-none data-[size=sm]:h-6 dark:bg-transparent dark:hover:bg-transparent dark:hover:text-foreground"
				>
					<Lock class="size-3.5 shrink-0 text-muted-foreground" />
					<span class="truncate">{current_permission.label}</span>
				</SelectTrigger>
			</div>
			<SelectContent class="rounded-2xl border-transparent bg-transparent p-0 shadow-none">
				<ShaderGlassSurface strength="strong" class="rounded-2xl p-1">
					<DropdownHoverSurface class="[--docs-sidebar-hover-radius:var(--radius-xl)]">
						{#snippet children({ move_hover })}
							{#each permission_options as option (option.id)}
								<Tooltip>
									<TooltipTrigger>
										{#snippet child({ props: tooltip_props })}
											<span {...tooltip_props} class="flex">
												<SelectItem
													value={option.id}
													class="w-full focus:bg-transparent! data-highlighted:bg-transparent! data-highlighted:text-foreground!"
													{@attach FollowHighlight(move_hover)}
												>
													{option.label}
												</SelectItem>
											</span>
										{/snippet}
									</TooltipTrigger>
									<TooltipContent side="right" class="max-w-56">
										{option.description}
									</TooltipContent>
								</Tooltip>
							{/each}
						{/snippet}
					</DropdownHoverSurface>
				</ShaderGlassSurface>
			</SelectContent>
		</Select>
	{/if}
</div>
