<script lang="ts">
	import { dev } from "$app/environment";

	interface LogoRow {
		line_height: number;
		size: number;
		text: string;
		tracking: number;
		weight: number;
	}

	interface SliderControl {
		readonly key: "size" | "line_height" | "tracking" | "weight";
		readonly label: string;
		readonly max: number;
		readonly min: number;
		readonly step: number;
		readonly unit: string;
	}

	const controls: ReadonlyArray<SliderControl> = [
		{ key: "size", label: "Font size", max: 400, min: 20, step: 1, unit: "px" },
		{ key: "line_height", label: "Line height", max: 2, min: 0.3, step: 0.01, unit: "" },
		{ key: "tracking", label: "Letter spacing", max: 0.5, min: -0.2, step: 0.005, unit: "em" },
		{ key: "weight", label: "Weight", max: 900, min: 100, step: 10, unit: "" },
	];

	const default_row = (text: string): LogoRow => ({
		line_height: 0.8,
		size: 180,
		text,
		tracking: -0.05,
		weight: 400,
	});

	let rows = $state([default_row("AR"), default_row("TIS"), default_row("AN")]);
	let text_color = $state("#ffffff");
	let background_color = $state("#8a2be2");
	let show_bounds = $state(false);
</script>

<svelte:head><title>Logo lab</title></svelte:head>

{#if !dev}
	<div class="flex h-full items-center justify-center p-10">
		<p class="text-sm text-muted-foreground">
			This surface belongs to development tooling and is not part of this build.
		</p>
	</div>
{:else}
	<!-- Above the Forge connection gate (z-50): this page needs no Forge at all. -->
	<div class="fixed inset-0 z-[60] flex flex-col bg-neutral-950">
		<div
			class="flex flex-1 items-center justify-center overflow-hidden"
			style:background-color={background_color}
		>
			<div
				class="flex flex-col items-center"
				style:color={text_color}
				style:font-family="'Sigurd Variable', serif"
			>
				{#each rows as row, index (index)}
					<div
						class="text-center whitespace-pre"
						style:outline={show_bounds ? "1px dashed rgb(255 255 255 / 0.4)" : "none"}
						style:font-size="{row.size}px"
						style:font-weight={row.weight}
						style:letter-spacing="{row.tracking}em"
						style:line-height={row.line_height}
					>{row.text}</div>
				{/each}
			</div>
		</div>

		<div class="flex flex-col gap-4 border-t border-neutral-800 p-4 text-neutral-200">
			<div class="flex flex-wrap items-center gap-6 text-xs">
				<label class="flex items-center gap-2">
					Color
					<input type="color" bind:value={text_color} class="h-6 w-10 cursor-pointer rounded" />
				</label>
				<label class="flex items-center gap-2">
					Background
					<input
						type="color"
						bind:value={background_color}
						class="h-6 w-10 cursor-pointer rounded"
					/>
				</label>
				<label class="flex items-center gap-2">
					<input type="checkbox" bind:checked={show_bounds} style="accent-color: #8b5cf6" />
					Show row bounds
				</label>
			</div>

			<!-- Inline grid: the running dev server may not have generated novel utilities yet. -->
			<div class="grid gap-6" style="grid-template-columns: repeat(3, minmax(0, 1fr))">
				{#each rows as row, index (index)}
					<div class="flex flex-col gap-2 rounded-lg border border-neutral-800 p-3">
						<input
							type="text"
							bind:value={row.text}
							aria-label="Row {index + 1} text"
							class="w-full rounded border border-neutral-700 bg-neutral-900 px-2 py-1 text-sm text-neutral-100"
						/>
						{#each controls as control (control.key)}
							<label class="flex items-center gap-2 text-xs">
								<span class="w-24 shrink-0 text-neutral-400">{control.label}</span>
								<input
									type="range"
									bind:value={row[control.key]}
									min={control.min}
									max={control.max}
									step={control.step}
									class="flex-1"
									style="accent-color: #8b5cf6"
								/>
								<span class="w-14 text-right tabular-nums">
									{row[control.key]}{control.unit}
								</span>
							</label>
						{/each}
					</div>
				{/each}
			</div>
		</div>
	</div>
{/if}
