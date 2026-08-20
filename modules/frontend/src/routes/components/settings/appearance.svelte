<script lang="ts" effect>
	import { Effect, Stream } from "effect";
	import RotateClockwise from "@tabler/icons-svelte/icons/rotate-clockwise";
	import {
		default_typography_preferences,
		resolve_typography_preferences,
		type TypographyFamily,
		type TypographyPreferences,
		type TypographyRole,
	} from "$lib/appearance/typography";
	import {
		prose_width,
		shader_enabled,
		typography,
		type ProseWidth,
	} from "$lib/appearance-config";
	import { BrowserTypography } from "$lib/browser/typography";
	import { Button } from "$lib/components/ui/button";
	import { Switch } from "$lib/components/ui/switch";
	import { ToggleGroup, ToggleGroupItem } from "$lib/components/ui/toggle-group";
	import {
		AppearancePreferences,
		type AppearanceState,
	} from "$lib/runtime/appearance-preferences";
	import Card from "./card.svelte";
	import FontPicker from "./font-picker.svelte";
	import Header from "./header.svelte";
	import Row from "./row.svelte";
	import Section from "./section.svelte";

	const preferences = yield* AppearancePreferences;
	const browser_typography = yield* BrowserTypography;
	const initial_stored = yield* preferences.Current;
	const initial_typography = resolve_typography_preferences(initial_stored);
	let stored = $state.raw<AppearanceState>(initial_stored);

	shader_enabled.set(initial_stored.shader_enabled);
	prose_width.set(initial_stored.prose_width ?? "balanced");
	typography.set(initial_typography);
	let enabled = $state(initial_stored.shader_enabled);
	let width = $state<ProseWidth>(initial_stored.prose_width ?? "balanced");
	let selected_typography = $state.raw<TypographyPreferences>(initial_typography);
	const ApplyStored = (next: AppearanceState) =>
		Effect.gen(function* () {
			const next_typography = resolve_typography_preferences(next);
			const typography_changed =
				next_typography.text !== selected_typography.text ||
				next_typography.code !== selected_typography.code;
			stored = next;
			enabled = next.shader_enabled;
			width = next.prose_width ?? "balanced";
			selected_typography = next_typography;
			shader_enabled.set(enabled);
			prose_width.set(width);
			typography.set(next_typography);
			if (typography_changed) {
				yield* browser_typography.Apply(next_typography).pipe(Effect.ignore);
			}
		});
	yield* preferences.Changes.pipe(Stream.runForEach(ApplyStored), Effect.forkScoped);
	yield* preferences.Load.pipe(Effect.forkScoped);
	const typography_is_default = $derived(
		selected_typography.text === default_typography_preferences.text &&
			selected_typography.code === default_typography_preferences.code,
	);

	const current_appearance = () => ({
		...stored,
		shader_enabled: enabled,
		prose_width: width,
		typography: selected_typography,
	});

	/**
	 * The store moves first so every glass surface answers the switch in the same
	 * frame; the durable write follows and is never what the reader waits on.
	 */
	const ToggleShader = (next: boolean) =>
		Effect.gen(function* () {
			enabled = next;
			shader_enabled.set(next);
			yield* preferences.Save(current_appearance());
		});

	const SelectProseWidth = (next: ProseWidth) =>
		Effect.gen(function* () {
			width = next;
			prose_width.set(next);
			yield* preferences.Save(current_appearance());
		});

	const SelectTypography = (role: TypographyRole, family: TypographyFamily) =>
		Effect.gen(function* () {
			selected_typography = { ...selected_typography, [role]: family };
			typography.set(selected_typography);
			yield* browser_typography.Apply(selected_typography).pipe(Effect.ignore);
			yield* preferences.Save(current_appearance());
		});

	const ResetTypography = Effect.gen(function* () {
		selected_typography = default_typography_preferences;
		typography.set(selected_typography);
		yield* browser_typography.Apply(selected_typography).pipe(Effect.ignore);
		yield* preferences.Save(current_appearance());
	});

	const widths: ReadonlyArray<{ label: string; value: ProseWidth }> = [
		{ label: "Tight", value: "tight" },
		{ label: "Balanced", value: "balanced" },
		{ label: "Loose", value: "loose" },
	];
</script>

<Header title="Appearance" description="How Artisan's surfaces are drawn." />

<Section id="typography" title="Typography">
	{#snippet action()}
		<Button
			variant="ghost"
			size="xs"
			disabled={typography_is_default}
			onclick={yield* ResetTypography}
		>
			<RotateClockwise />
			Restore defaults
		</Button>
	{/snippet}

	<Card class="mt-3">
		<div class="grid divide-y divide-border/40 sm:grid-cols-2 sm:divide-x sm:divide-y-0">
			<div class="min-w-0 px-4 py-4">
				<span class="font-sans text-[0.625rem] font-medium tracking-[0.14em] text-muted-foreground uppercase">Text</span>
				<p class="mt-2 truncate font-sans text-sm leading-tight text-foreground">Quiet tools, clear decisions.</p>
			</div>
			<div class="min-w-0 px-4 py-4">
				<span class="font-sans text-[0.625rem] font-medium tracking-[0.14em] text-muted-foreground uppercase">Code</span>
				<p class="mt-2 truncate font-mono text-xs leading-tight text-foreground">craft = deliberate</p>
			</div>
		</div>

		<Row title="Text" description="Interface controls, page titles, and reading text.">
			{#snippet control()}
				<FontPicker
					role="text"
					family={selected_typography.text}
					onselect={(family) => SelectTypography("text", family)}
				/>
			{/snippet}
		</Row>
		<Row title="Code" description="Code, terminals, and the editor.">
			{#snippet control()}
				<FontPicker
					role="code"
					family={selected_typography.code}
					onselect={(family) => SelectTypography("code", family)}
				/>
			{/snippet}
		</Row>
	</Card>
	<p class="mt-2.5 max-w-2xl text-pretty text-xs leading-relaxed text-muted-foreground">
		Local font names are requested only when you open a picker. The list stays on this
		device; Artisan saves only the two family names you choose.
	</p>
</Section>

<Section id="glass" title="Glass">
	<Card class="mt-3">
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
	</Card>
</Section>

<Section id="reading" title="Reading">
	<Card class="mt-3">
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
	</Card>
</Section>
