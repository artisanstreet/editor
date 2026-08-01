<script lang="ts" effect>
	import ArrowsHorizontal from "@tabler/icons-svelte/icons/arrows-horizontal";
	import BoltFilled from "@tabler/icons-svelte/icons/bolt-filled";
	import Brain from "@tabler/icons-svelte/icons/brain";
	import Lock from "@tabler/icons-svelte/icons/lock";
	import { Effect } from "effect";
	import type { ThreadSessionPolicy } from "@artisan/protocol";
	import {
		thinking_level_labels,
		type ContextWindowChoice,
		type ModelChoice,
		type PermissionOption,
		type SpeedOption,
		type ThinkingLevel,
	} from "$lib/engine/model-selection";

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

	const SelectThinking = (event: Event) =>
		Effect.gen(function* () {
			if (!(event.currentTarget instanceof HTMLSelectElement)) return;
			if (thinking.availability !== "supported") return;
			const level = thinking.options.find((option) => option.id === event.currentTarget.value)?.id;
			if (level !== undefined) yield* onthinking(model, level);
		});

	const SelectSpeed = (event: Event) =>
		Effect.gen(function* () {
			if (!(event.currentTarget instanceof HTMLSelectElement)) return;
			const option = speeds.find((candidate) => candidate.id === event.currentTarget.value);
			if (option !== undefined) yield* onspeed(model, option);
		});

	const SelectContext = (event: Event) =>
		Effect.gen(function* () {
			if (!(event.currentTarget instanceof HTMLSelectElement) || context === undefined) return;
			const option = context.options.find((candidate) => candidate.id === event.currentTarget.value);
			if (option !== undefined) yield* oncontext(model, option);
		});

	const SelectPermission = (event: Event) =>
		Effect.gen(function* () {
			if (!(event.currentTarget instanceof HTMLSelectElement)) return;
			const option = permission_options.find(
				(candidate) => candidate.id === event.currentTarget.value,
			);
			if (option !== undefined) yield* onpermission(model, option);
		});
</script>

{#snippet control(Icon, label, value, options, onchange)}
	<label class="card relative flex h-6 min-w-0 items-center rounded-md bg-linear-to-b from-surface-225 to-surface-200 px-2 text-xs dark:from-surface-800 dark:to-surface-925">
		<Icon class="pointer-events-none size-3.5 shrink-0 text-muted-foreground" />
		<span class="pointer-events-none ml-1.5 min-w-0 truncate text-foreground">{label}</span>
		<select
			class="absolute inset-0 size-full cursor-pointer opacity-0 disabled:cursor-not-allowed"
			{value}
			{disabled}
			onchange={yield* onchange(event)}
			aria-label={label}
		>
			{#each options as option (option.id)}
				<option value={option.id}>{option.label}</option>
			{/each}
		</select>
	</label>
{/snippet}

<div class="flex flex-col gap-1.5">
	{#if thinking.availability === "supported" && current_thinking !== undefined}
		{@render control(
			Brain,
			thinking_level_labels[current_thinking],
			current_thinking,
			thinking.options.map((option) => ({
				id: option.id,
				label: thinking_level_labels[option.id],
			})),
			SelectThinking,
		)}
	{/if}

	{#if speeds.length > 1 && current_speed !== undefined}
		{@render control(BoltFilled, current_speed.label, current_speed.id, speeds, SelectSpeed)}
	{/if}

	{#if context !== undefined && current_context !== undefined}
		{@render control(
			ArrowsHorizontal,
			current_context.label,
			current_context.id,
			context.options,
			SelectContext,
		)}
	{/if}

	{#if permission_options.length > 1 && current_permission !== undefined}
		{@render control(
			Lock,
			current_permission.label,
			current_permission.id,
			permission_options,
			SelectPermission,
		)}
	{/if}
</div>
