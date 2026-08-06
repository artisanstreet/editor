<script lang="ts" effect>
	import { Effect } from "effect";
	import { prose_width, shader_enabled, type ProseWidth } from "$lib/appearance-config";
	import { Switch } from "$lib/components/ui/switch";
	import { ToggleGroup, ToggleGroupItem } from "$lib/components/ui/toggle-group";
	import { AppearancePreferences } from "$lib/runtime/appearance-preferences";
	import Row from "./row.svelte";

	const preferences = yield* AppearancePreferences;
	const stored = yield* preferences.Load;

	shader_enabled.set(stored.shader_enabled);
	prose_width.set(stored.prose_width ?? "balanced");
	let enabled = $state(stored.shader_enabled);
	let width = $state<ProseWidth>(stored.prose_width ?? "balanced");

	/**
	 * The store moves first so every glass surface answers the switch in the same
	 * frame; the durable write follows and is never what the reader waits on.
	 */
	const ToggleShader = (next: boolean) =>
		Effect.gen(function* () {
			enabled = next;
			shader_enabled.set(next);
			yield* preferences.Save({ ...stored, shader_enabled: enabled, prose_width: width });
		});

	const SelectProseWidth = (next: ProseWidth) =>
		Effect.gen(function* () {
			width = next;
			prose_width.set(next);
			yield* preferences.Save({ ...stored, shader_enabled: enabled, prose_width: width });
		});

	const widths: ReadonlyArray<{ label: string; value: ProseWidth }> = [
		{ label: "Tight", value: "tight" },
		{ label: "Balanced", value: "balanced" },
		{ label: "Loose", value: "loose" },
	];
</script>

<h1 class="text-lg font-semibold text-foreground">Appearance</h1>
<p class="mt-1 text-sm text-muted-foreground">How Artisan's surfaces are drawn.</p>

<section class="mt-10" aria-labelledby="glass">
	<h2 id="glass" class="scroll-mt-6 text-sm font-medium text-foreground">Glass</h2>
	<div
		class="card mt-3 rounded-xl bg-linear-to-b from-surface-225 to-surface-200 dark:from-surface-800 dark:to-surface-925"
	>
		<div class="flex flex-col divide-y divide-border/40">
			<Row
				title="Shader under glass"
				description="Lights glass surfaces with the animated shader they were designed around. Turning it off leaves the glass itself intact — the material, highlight, and depth stay — and only the moving light stops."
			>
				{#snippet control()}
					<Switch
						checked={enabled}
						aria-label="Shader under glass"
						onclick={yield* ToggleShader(!enabled)}
					/>
				{/snippet}
			</Row>
		</div>
	</div>
</section>

<section class="mt-10" aria-labelledby="reading">
	<h2 id="reading" class="scroll-mt-6 text-sm font-medium text-foreground">Reading</h2>
	<div
		class="card mt-3 rounded-xl bg-linear-to-b from-surface-225 to-surface-200 dark:from-surface-800 dark:to-surface-925"
	>
		<div class="flex flex-col divide-y divide-border/40">
			<Row
				title="Prose width"
				description="How wide the transcript's reading column runs. Balanced is the width Artisan was designed at; Tight shortens the line for focus, Loose spends more of the window on text."
			>
				{#snippet control()}
					<ToggleGroup type="single" value={width} variant="outline" aria-label="Prose width">
						{#each widths as option (option.value)}
							<ToggleGroupItem
								value={option.value}
								aria-label={option.label}
								onclick={yield* SelectProseWidth(option.value)}
							>
								{option.label}
							</ToggleGroupItem>
						{/each}
					</ToggleGroup>
				{/snippet}
			</Row>
		</div>
	</div>
</section>
