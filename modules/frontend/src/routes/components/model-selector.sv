<script lang="ts">
	import ChevronDown from "@tabler/icons-svelte/icons/chevron-down";
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
</script>

<Popover bind:open>
	<PopoverTrigger
		aria-label="Select model"
		class="group/model-selector flex w-full flex-row items-center gap-4 rounded-xl p-0 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
	>
		{@const SelectedIcon = selected_engine.icon}
		<SelectedIcon
			class={selected_engine.monochrome
				? "size-8 shrink-0 dark:invert"
				: "size-8 shrink-0"}
		/>
		<div class="flex min-w-0 flex-1 flex-col -space-y-1">
			<span class="truncate text-lg font-semibold text-foreground">{selected_model.name}</span>
			<span class="truncate text-sm text-muted-foreground">{selected_model.lab}</span>
		</div>
		<ChevronDown
			class="size-4 shrink-0 text-muted-foreground transition-transform group-data-[state=open]/model-selector:rotate-180"
		/>
	</PopoverTrigger>

	<PopoverContent
		align="end"
		sideOffset={8}
		class="w-[min(32rem,calc(100vw-2rem))] gap-2 overflow-hidden p-2"
	>
		<Tabs bind:value={active_engine} class="min-h-0 gap-2">
			<TabsList variant="line" aria-label="Coding engines" class="w-full justify-start overflow-x-auto px-1">
				{#each engines as engine (engine.id)}
					{@const EngineIcon = engine.icon}
					<TabsTrigger
						value={engine.id}
						aria-label={engine.name}
						title={engine.name}
						class="size-9 flex-none px-0"
					>
						<EngineIcon class={engine.monochrome ? "size-5 dark:invert" : "size-5"} />
					</TabsTrigger>
				{/each}
			</TabsList>

			<ScrollArea class="h-64 rounded-xl">
				<table class="w-full border-separate border-spacing-y-1" aria-label="Available models">
					<tbody>
						{#each active_models as model (model.id)}
							{@const ModelIcon = engines.find((engine) => engine.id === model.engine)?.icon ?? SvglOpenAILogo}
							{@const is_monochrome = engines.find((engine) => engine.id === model.engine)?.monochrome ?? false}
							<tr>
								<td class="p-0">
									<button
										type="button"
										class="flex w-full items-center gap-3 rounded-xl px-3 py-2 text-left hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none"
										aria-current={model.id === selected_model_id ? "true" : undefined}
										onclick={() => select_model(model)}
									>
										<ModelIcon
											class={is_monochrome
												? "size-6 shrink-0 dark:invert"
												: "size-6 shrink-0"}
										/>
										<span class="flex min-w-0 flex-col -space-y-1">
											<span class="truncate text-sm font-semibold text-foreground">{model.name}</span>
											<span class="truncate text-xs text-muted-foreground">{model.lab}</span>
										</span>
									</button>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</ScrollArea>
		</Tabs>
	</PopoverContent>
</Popover>
