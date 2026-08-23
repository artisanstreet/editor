<script lang="ts" effect>
	import {
		default_desktop_app_icon,
		type DesktopAppIconPreference,
	} from "@artisan/protocol";
	import { Effect, Stream } from "effect";
	import RotateClockwise from "@tabler/icons-svelte/icons/rotate-clockwise";
	import foreground_gradient_icon from "$lib/assets/barekey/runtime-app-icons/foreground-gradient-symbol.png";
	import plastic_jaw_icon from "$lib/assets/barekey/runtime-app-icons/plastic-jaw-shading.png";
	import {
		default_typography_preferences,
		resolve_typography_preferences,
		type TypographyFamily,
		type TypographyPreferences,
		type TypographyRole,
	} from "$lib/appearance/typography";
	import {
		PathSeparatorCharacter,
		ResolveDisplayFormatPreferences,
		type PathSeparator,
		type TimeFormat,
	} from "$lib/appearance/display-format";
	import {
		path_separator,
		prose_width,
		shader_enabled,
		time_format,
		typography,
		type ProseWidth,
	} from "$lib/appearance-config";
	import { BrowserTypography } from "$lib/browser/typography";
	import { RunBrowserDom } from "$lib/browser/dom";
	import {
		DesktopAppIconsAvailable,
		LoadDesktopAppIcon,
		SelectDesktopAppIcon,
	} from "$lib/browser/desktop-app-icon";
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
	const initial_display = ResolveDisplayFormatPreferences(initial_stored);
	let stored = $state.raw<AppearanceState>(initial_stored);

	shader_enabled.set(initial_stored.shader_enabled);
	prose_width.set(initial_stored.prose_width ?? "balanced");
	path_separator.set(initial_display.path_separator);
	time_format.set(initial_display.time_format);
	typography.set(initial_typography);
	let enabled = $state(initial_stored.shader_enabled);
	let width = $state<ProseWidth>(initial_stored.prose_width ?? "balanced");
	let selected_typography = $state.raw<TypographyPreferences>(initial_typography);
	let selected_time_format = $state<TimeFormat>(initial_display.time_format);
	let selected_path_separator = $state<PathSeparator>(initial_display.path_separator);
	const desktop_app_icons_available = yield* RunBrowserDom(() =>
		DesktopAppIconsAvailable(globalThis.location?.protocol ?? ""),
	);
	let selected_app_icon = $state<DesktopAppIconPreference>(default_desktop_app_icon);
	if (desktop_app_icons_available) {
		selected_app_icon = yield* LoadDesktopAppIcon.pipe(
			Effect.catch(() => Effect.succeed(default_desktop_app_icon)),
		);
	}
	const ApplyStored = (next: AppearanceState) =>
		Effect.gen(function* () {
			const next_typography = resolve_typography_preferences(next);
			const next_display = ResolveDisplayFormatPreferences(next);
			const typography_changed =
				next_typography.text !== selected_typography.text ||
				next_typography.code !== selected_typography.code;
			stored = next;
			enabled = next.shader_enabled;
			width = next.prose_width ?? "balanced";
			selected_typography = next_typography;
			selected_time_format = next_display.time_format;
			selected_path_separator = next_display.path_separator;
			shader_enabled.set(enabled);
			prose_width.set(width);
			time_format.set(selected_time_format);
			path_separator.set(selected_path_separator);
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
		path_separator: selected_path_separator,
		time_format: selected_time_format,
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

	const SelectTimeFormat = (next: TimeFormat) =>
		Effect.gen(function* () {
			selected_time_format = next;
			time_format.set(next);
			yield* preferences.Save(current_appearance());
		});

	const SelectPathSeparator = (next: PathSeparator) =>
		Effect.gen(function* () {
			selected_path_separator = next;
			path_separator.set(next);
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

	const SelectAppIcon = (icon: DesktopAppIconPreference) =>
		Effect.gen(function* () {
			if (!desktop_app_icons_available || icon === selected_app_icon) return;
			yield* SelectDesktopAppIcon(icon);
			selected_app_icon = icon;
		});

	const app_icons: ReadonlyArray<{
		image: string;
		label: string;
		value: DesktopAppIconPreference;
	}> = [
		{
			image: plastic_jaw_icon,
			label: "Plastic + jaw shading",
			value: "plastic-jaw-shading",
		},
		{
			image: foreground_gradient_icon,
			label: "Foreground plastic + gradient symbol",
			value: "foreground-gradient-symbol",
		},
	];

	const widths: ReadonlyArray<{ label: string; value: ProseWidth }> = [
		{ label: "Tight", value: "tight" },
		{ label: "Balanced", value: "balanced" },
		{ label: "Loose", value: "loose" },
	];
	const time_formats: ReadonlyArray<{ label: string; value: TimeFormat }> = [
		{ label: "12-hour", value: "12-hour" },
		{ label: "24-hour", value: "24-hour" },
	];
	const path_separators: ReadonlyArray<{
		label: string;
		value: PathSeparator;
	}> = [
		{ label: "Backslash", value: "backslash" },
		{ label: "Forward slash", value: "forward-slash" },
	];
</script>

<Header title="Appearance" description="How Artisan's surfaces are drawn." />

<Section id="app-icon" title="App icon">
	<Card class="mt-3">
		<div class="grid gap-2 p-3 sm:grid-cols-2">
			{#each app_icons as option (option.value)}
				<button
					type="button"
					aria-pressed={selected_app_icon === option.value}
					disabled={!desktop_app_icons_available}
					onclick={yield* SelectAppIcon(option.value)}
					class={selected_app_icon === option.value
						? "flex min-w-0 items-center gap-3 rounded-xl border border-foreground/25 bg-foreground/6 p-3 text-left outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-55"
						: "flex min-w-0 items-center gap-3 rounded-xl border border-transparent p-3 text-left outline-none transition-colors hover:border-border hover:bg-muted/45 focus-visible:ring-2 focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-55"}
				>
					<img
						alt=""
						aria-hidden="true"
						class="size-16 shrink-0 object-contain"
						src={option.image}
					/>
					<span class="flex min-w-0 flex-col gap-1">
						<span class="text-pretty text-sm leading-tight font-medium text-foreground">
							{option.label}
						</span>
						<span class="text-xs text-muted-foreground">
							{option.value === default_desktop_app_icon ? "Default" : "Alternate"}
						</span>
					</span>
				</button>
			{/each}
		</div>
	</Card>
	<p class="mt-2.5 max-w-2xl text-pretty text-xs leading-relaxed text-muted-foreground">
		{desktop_app_icons_available
			? "Changes the running window, taskbar, or Dock icon immediately and remembers the choice for the next launch."
			: "Open Appearance in the Artisan desktop app to switch its runtime icon."}
	</p>
</Section>

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

<Section id="formatting" title="Formatting">
	<Card class="mt-3">
		<Row
			title="Time format"
			description="How local times are written throughout Artisan."
		>
			{#snippet control()}
				<ToggleGroup
					type="single"
					value={selected_time_format}
					variant="outline"
					aria-label="Time format"
				>
					{#each time_formats as option (option.value)}
						<ToggleGroupItem
							value={option.value}
							aria-label={option.label}
							onclick={yield* SelectTimeFormat(option.value)}
						>
							{option.label}
						</ToggleGroupItem>
					{/each}
				</ToggleGroup>
			{/snippet}
		</Row>
		<Row
			title="Path separator"
			description="Which separator file and folder paths use when displayed."
		>
			{#snippet control()}
				<ToggleGroup
					type="single"
					value={selected_path_separator}
					variant="outline"
					aria-label="Path separator"
				>
					{#each path_separators as option (option.value)}
						<ToggleGroupItem
							value={option.value}
							aria-label={option.label}
							onclick={yield* SelectPathSeparator(option.value)}
						>
							<span class="font-mono">{PathSeparatorCharacter(option.value)}</span>
						</ToggleGroupItem>
					{/each}
				</ToggleGroup>
			{/snippet}
		</Row>
	</Card>
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
