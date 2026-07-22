<script lang="ts">
	import Selector from "@tabler/icons-svelte/icons/selector";
	import {
		SvglClaudeAILogo,
		SvglGoogleAntigravityLogo,
		SvglGrokLogo,
		SvglOpenAILogo,
	} from "@selemondev/svgl-svelte";
	import type { Component } from "svelte";

	import { Popover, PopoverContent, PopoverTrigger } from "$lib/components/ui/popover";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import { Tabs, TabsList, TabsTrigger } from "$lib/components/ui/tabs";
	import OpenCodeIcon from "./opencode-icon.sv";

	type EngineId = "codex" | "claude" | "grok" | "opencode" | "antigravity";

	type Engine = {
		id: EngineId;
		name: string;
		icon: Component;
		monochrome: boolean;
	};

	type ModelChoice = {
		id: string;
		engine: EngineId;
		name: string;
		lab: string;
	};

	const engines: ReadonlyArray<Engine> = [
		{ id: "codex", name: "Codex", icon: SvglOpenAILogo, monochrome: true },
		{ id: "claude", name: "Claude Code", icon: SvglClaudeAILogo, monochrome: false },
		{ id: "grok", name: "Grok", icon: SvglGrokLogo, monochrome: true },
		{ id: "opencode", name: "OpenCode", icon: OpenCodeIcon, monochrome: false },
		{
			id: "antigravity",
			name: "Antigravity",
			icon: SvglGoogleAntigravityLogo,
			monochrome: false,
		},
	];

	const models: ReadonlyArray<ModelChoice> = [
		{ id: "codex-sol", engine: "codex", name: "GPT 5.6 Sol", lab: "Codex" },
		{ id: "codex-terra", engine: "codex", name: "GPT 5.6 Terra", lab: "Codex" },
		{ id: "codex-luna", engine: "codex", name: "GPT 5.6 Luna", lab: "Codex" },
		{ id: "claude-opus", engine: "claude", name: "Claude Opus 4.6", lab: "Claude Code" },
		{ id: "claude-sonnet", engine: "claude", name: "Claude Sonnet 4.6", lab: "Claude Code" },
		{ id: "grok-4-5", engine: "grok", name: "Grok 4.5", lab: "Grok" },
		{ id: "opencode-zen", engine: "opencode", name: "OpenCode Zen", lab: "OpenCode" },
		{ id: "opencode-any", engine: "opencode", name: "Configured model", lab: "OpenCode" },
		{
			id: "antigravity-flash",
			engine: "antigravity",
			name: "Gemini 3.5 Flash High",
			lab: "Antigravity",
		},
		{
			id: "antigravity-pro",
			engine: "antigravity",
			name: "Gemini 3.1 Pro High",
			lab: "Antigravity",
		},
	];

	let open = $state(false);
	let active_engine = $state<EngineId>("codex");
	let selected_model_id = $state("codex-sol");
	let model_surface = $state<HTMLElement>();
	let hover_animated = $state(false);
	let hover_height = $state(0);
	let hover_left = $state(0);
	let hover_top = $state(0);
	let hover_visible = $state(false);
	let hover_width = $state(0);
	let engine_surface = $state<HTMLElement | null>(null);
	let engine_indicator_animated = $state(false);
	let engine_indicator_left = $state(0);
	let engine_indicator_visible = $state(false);
	let engine_indicator_width = $state(0);

	const active_models = $derived(models.filter((model) => model.engine === active_engine));
	const selected_model = $derived(models.find((model) => model.id === selected_model_id) ?? models[0]);
	const selected_engine = $derived(
		engines.find((engine) => engine.id === selected_model.engine) ?? engines[0],
	);

	const select_model = (model: ModelChoice) => {
		selected_model_id = model.id;
		active_engine = model.engine;
		open = false;
	};

	const clear_hover = () => {
		hover_animated = false;
		hover_visible = false;
	};

	const move_hover = (event: Event) => {
		if (!(event.currentTarget instanceof HTMLElement) || !model_surface) {
			return;
		}

		const surface_rect = model_surface.getBoundingClientRect();
		const target_rect = event.currentTarget.getBoundingClientRect();

		hover_animated = hover_visible;
		hover_height = target_rect.height;
		hover_left = target_rect.left - surface_rect.left;
		hover_top = target_rect.top - surface_rect.top;
		hover_visible = true;
		hover_width = target_rect.width;
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
		void active_engine;
		void engine_surface;

		const frame = requestAnimationFrame(() => position_engine_indicator(true));

		return () => cancelAnimationFrame(frame);
	});
</script>

<svelte:window onresize={() => position_engine_indicator(false)} />

<Popover bind:open>
	<PopoverTrigger
		aria-label="Select model"
		class="flex w-full flex-row items-center gap-4 rounded-2xl bg-linear-to-b from-foreground/7.5 to-foreground/2.5 p-2 pl-4 text-left text-muted-foreground outline-none transition-colors card-lg hover:text-foreground focus-visible:text-foreground focus-visible:ring-[3px] focus-visible:ring-ring/50"
	>
		{@const SelectedIcon = selected_engine.icon}
		<SelectedIcon
			class={selected_engine.monochrome
				? "size-6 shrink-0 dark:invert"
				: "size-6 shrink-0"}
		/>
		<div class="flex min-w-0 flex-1 flex-col -space-y-1">
			<span class="truncate text-base font-semibold text-foreground">{selected_model.name}</span>
			<span class="truncate text-xs text-muted-foreground">{selected_model.lab}</span>
		</div>
		<Selector class="pointer-events-none size-4 shrink-0 text-muted-foreground" />
	</PopoverTrigger>

	<PopoverContent
		align="end"
		sideOffset={8}
		class="w-[min(20rem,calc(100vw-2rem))] gap-2 overflow-hidden bg-background p-2"
	>
		<Tabs bind:value={active_engine} class="min-h-0 gap-2">
			<TabsList
				bind:ref={engine_surface}
				variant="line"
				aria-label="Coding engines"
				class="card relative h-auto! w-full justify-start overflow-x-auto rounded-lg! bg-linear-to-b from-foreground/10 to-foreground/5 p-1"
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
					<TabsTrigger
						value={engine.id}
						data-engine={engine.id}
						aria-label={engine.name}
						title={engine.name}
						class="relative z-1 size-8 flex-none px-0 text-foreground after:hidden hover:text-foreground data-active:border-transparent data-active:bg-transparent data-active:text-foreground dark:hover:text-foreground dark:data-active:border-transparent dark:data-active:bg-transparent"
					>
						<EngineIcon class={engine.monochrome ? "size-4 dark:invert" : "size-4"} />
					</TabsTrigger>
				{/each}
			</TabsList>

			<ScrollArea class="h-48 rounded-xl">
				<div bind:this={model_surface} class="relative" role="presentation" onpointerleave={clear_hover}>
					<div
						class="docs-sidebar-hover-highlight"
						data-active={hover_visible}
						data-animate={hover_animated}
						aria-hidden="true"
						style={`--docs-sidebar-hover-x: ${hover_left}px; --docs-sidebar-hover-y: ${hover_top}px; --docs-sidebar-hover-width: ${hover_width}px; --docs-sidebar-hover-height: ${hover_height}px;`}
					></div>
				<table class="relative z-1 w-full border-separate border-spacing-y-0.5" aria-label="Available models">
					<tbody>
						{#each active_models as model (model.id)}
							{@const ModelIcon = engines.find((engine) => engine.id === model.engine)?.icon ?? SvglOpenAILogo}
							{@const is_monochrome = engines.find((engine) => engine.id === model.engine)?.monochrome ?? false}
							<tr>
								<td class="p-0">
									<button
										type="button"
										class="flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none"
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
										</span>
									</button>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
				</div>
			</ScrollArea>
		</Tabs>
	</PopoverContent>
</Popover>

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

	.model-selector-engine-light::before {
		position: absolute;
		top: 0;
		left: 50%;
		width: 0.625rem;
		height: 0.25rem;
		content: "";
		background: linear-gradient(
			to bottom,
			oklch(from var(--foreground) l c h / 92%),
			oklch(from var(--foreground) l c h / 42%)
		);
		clip-path: polygon(0 0, 100% 0, 50% 100%);
		filter: drop-shadow(0 0.125rem 0.25rem oklch(from var(--foreground) l c h / 62%));
		transform: translateX(-50%);
	}

	.model-selector-engine-light::after {
		position: absolute;
		top: 0;
		left: 50%;
		width: 2.75rem;
		height: 100%;
		content: "";
		background: radial-gradient(
			ellipse at 50% 0%,
			oklch(from var(--foreground) l c h / 24%) 0%,
			oklch(from var(--foreground) l c h / 8%) 38%,
			transparent 74%
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
</style>
