<script lang="ts">
	import { dev } from "$app/environment";
	import PlayerTrackNext from "@tabler/icons-svelte/icons/player-track-next";
	import PlayerTrackPrev from "@tabler/icons-svelte/icons/player-track-prev";
	import { Button } from "$lib/components/ui/button";
	import ForgeShellPreview from "$/components/forge-shell-preview.sv";

	const banner = `
   █████████              █████     ███                                  ███████████
  ███▒▒▒▒▒███            ▒▒███     ▒▒▒                                  ▒▒███▒▒▒▒▒▒█
 ▒███    ▒███  ████████  ███████   ████   █████   ██████   ████████      ▒███   █ ▒   ██████  ████████   ███████  ██████
 ▒███████████ ▒▒███▒▒███▒▒▒███▒   ▒▒███  ███▒▒   ▒▒▒▒▒███ ▒▒███▒▒███     ▒███████    ███▒▒███▒▒███▒▒███ ███▒▒███ ███▒▒███
 ▒███▒▒▒▒▒███  ▒███ ▒▒▒   ▒███     ▒███ ▒▒█████   ███████  ▒███ ▒███     ▒███▒▒▒█   ▒███ ▒███ ▒███ ▒▒▒ ▒███ ▒███▒███████
 ▒███    ▒███  ▒███       ▒███ ███ ▒███  ▒▒▒▒███ ███▒▒███  ▒███ ▒███     ▒███  ▒    ▒███ ▒███ ▒███     ▒███ ▒███▒███▒▒▒
 █████   █████ █████      ▒▒█████  █████ ██████ ▒▒████████ ████ █████    █████      ▒▒██████  █████    ▒▒███████▒▒██████
▒▒▒▒▒   ▒▒▒▒▒ ▒▒▒▒▒        ▒▒▒▒▒  ▒▒▒▒▒ ▒▒▒▒▒▒   ▒▒▒▒▒▒▒▒ ▒▒▒▒ ▒▒▒▒▒    ▒▒▒▒▒        ▒▒▒▒▒▒  ▒▒▒▒▒      ▒▒▒▒▒███ ▒▒▒▒▒▒
                                                                                                        ███ ▒███
                                                                                                       ▒▒██████
                                                                                                        ▒▒▒▒▒▒`;

	/** Tailwind hue shades 400/500/600, as literal values for inline styles. */
	const blue_shades = ["#60a5fa", "#3b82f6", "#2563eb"];
	const green_shades = ["#4ade80", "#22c55e", "#16a34a"];
	const red_shades = ["#f87171", "#ef4444", "#dc2626"];

	interface BannerScenario {
		/** The specialized card utility carrying this scheme's tinted shadow ring. */
		readonly card: string;
		readonly id: string;
		readonly label: string;
		/** One plain sentence, shown in the banner's bottom-left corner. */
		readonly message: string;
		/** Wave shades, top of the box down to its base. */
		readonly shades: ReadonlyArray<string>;
	}

	const scenarios: ReadonlyArray<BannerScenario> = [
		{
			card: "card-info",
			id: "connecting",
			label: "Connecting",
			message: "Connecting to your Forge…",
			shades: blue_shades,
		},
		{
			card: "card-info",
			id: "reconnecting",
			label: "Reconnecting",
			message: "Connection dropped — getting you back…",
			shades: blue_shades,
		},
		{
			card: "card-success",
			id: "connected",
			label: "Connected",
			message: "You're connected.",
			shades: green_shades,
		},
		{
			card: "card-error",
			id: "disconnected",
			label: "Disconnected",
			message: "Lost touch with Forge.",
			shades: red_shades,
		},
		{
			card: "card-error",
			id: "stopped",
			label: "Stopped",
			message: "Forge isn't running right now.",
			shades: red_shades,
		},
	];

	let index = $state(0);
	/** How many bands the scheme's colour journey is sliced into. */
	let slices = $state(6);
	const scenario = $derived(scenarios[index]);
	const cycle = (delta: number) => {
		index = (index + delta + scenarios.length) % scenarios.length;
	};

	/** Linear RGB interpolation is plenty for a mock; the stops are all literal hex. */
	const hex_channel = (hex: string, offset: number) =>
		Number.parseInt(hex.slice(offset, offset + 2), 16);
	const mix_hex = (from: string, to: string, amount: number) => {
		const channel = (offset: number) =>
			Math.round(
				hex_channel(from, offset) +
					(hex_channel(to, offset) - hex_channel(from, offset)) * amount,
			);
		return `rgb(${channel(1)} ${channel(3)} ${channel(5)})`;
	};
	/** Resamples the scheme's stops into `count` evenly spaced shades. */
	const slice_shades = (stops: ReadonlyArray<string>, count: number) => {
		const first = stops[0];
		if (first === undefined) return [];
		if (stops.length === 1 || count === 1) return Array.from({ length: count }, () => first);
		return Array.from({ length: count }, (_, slice) => {
			const position = (slice / (count - 1)) * (stops.length - 1);
			const lower = Math.min(Math.floor(position), stops.length - 2);
			return mix_hex(stops[lower] ?? first, stops[lower + 1] ?? first, position - lower);
		});
	};

	/**
	 * A pixel-art take on the rising status gradient: a coarse grid of wide
	 * rectangles in three shade bands (hue 600 at the base, 500, then 400). Each
	 * band edge undulates like a wave — two out-of-phase sines per band — and
	 * the swell is clamped to about one row so the unevenness never degenerates
	 * into scattered noise.
	 */
	const dither_columns = 24;
	/** Rows follow the slice count so every band keeps at least one row of its own. */
	const dither_rows = $derived(Math.max(12, slices));
	const wave_clamp = 1.1;
	const band_edge = (column: number, band: number) => {
		const swell =
			Math.sin(column * 0.52 + band * 2.1) * 0.75 +
			Math.sin(column * 0.21 + band * 4.7) * 0.55;
		return Math.max(-wave_clamp, Math.min(wave_clamp, swell));
	};
	/**
	 * One colour per cell. The scheme's shades split the height into equal
	 * bands; only the interior waterlines wave, so the box stays fully coloured
	 * to its edges however many shades a scheme brings.
	 */
	const dither_cells = $derived.by(() => {
		const shades = slice_shades(scenario?.shades ?? [], slices);
		const bands = shades.length;
		/** Thin bands get gentler swell so a wave never swallows its neighbour. */
		const swell_limit = Math.min(wave_clamp, dither_rows / bands / 2);
		return Array.from({ length: dither_rows * dither_columns }, (_, cell) => {
			const row = Math.floor(cell / dither_columns);
			const column = cell % dither_columns;
			let shade_index = 0;
			for (let edge = 0; edge < bands - 1; edge += 1) {
				const swell = Math.max(
					-swell_limit,
					Math.min(swell_limit, band_edge(column, edge)),
				);
				const edge_row = (dither_rows * (edge + 1)) / bands + swell;
				if (row >= edge_row) shade_index += 1;
			}
			return shades[Math.min(shade_index, bands - 1)] ?? "transparent";
		});
	});
</script>

<!--
	Unguarded because it cannot be otherwise: `svelte:head` may not sit inside a
	block. The production build stubs this whole file, so the name never ships.
-->
<svelte:head><title>Forge banner states</title></svelte:head>

{#if !dev}
	<div class="flex h-full items-center justify-center p-10">
		<p class="text-sm text-muted-foreground">
			This surface belongs to development tooling and is not part of this build.
		</p>
	</div>
{:else if scenario !== undefined}
	<div class="relative h-full min-h-0 overflow-hidden bg-background">
		<!-- The same skeleton the real gate covers, so the blur has true shell content behind it. -->
		<ForgeShellPreview />

		<!-- Fixed, not absolute: the blur must cover the whole screen, panel chrome and all. -->
		<div
			class="fixed inset-0 z-50 grid place-items-center bg-background/45 px-6 backdrop-blur-md supports-[backdrop-filter]:bg-background/35"
		>
			<div class="relative flex max-w-full flex-col items-center gap-8">
				<div
					class={`relative max-w-full overflow-hidden rounded-xl px-12 py-8 ${scenario.card}`}
				>
					<div
						class="absolute inset-0 grid"
						aria-hidden="true"
						style={`grid-template-columns: repeat(${dither_columns}, 1fr); grid-template-rows: repeat(${dither_rows}, 1fr)`}
					>
						{#each dither_cells as shade, cell (cell)}
							<div style={`background-color: ${shade}`}></div>
						{/each}
					</div>
					<div class="relative flex max-w-full flex-col gap-0">
						<pre
							class="banner-art max-w-full overflow-hidden text-foreground select-none"
							aria-label="Artisan Forge">{banner}</pre>

						<div class="text-sm" role="status" aria-live="polite">
							<span class="status-shimmer">{scenario.message}</span>
						</div>
					</div>
				</div>
			</div>
		</div>

		<div class="fixed top-4 right-4 z-[60] flex flex-col items-end gap-2">
			<div
				class="card flex items-center gap-1 rounded-full bg-linear-to-b from-surface-225 to-surface-200 p-1 dark:from-surface-800 dark:to-surface-925"
			>
				<Button
					variant="ghost"
					size="icon-sm"
					aria-label="Previous status"
					onclick={() => cycle(-1)}
				>
					<PlayerTrackPrev />
				</Button>
				<span class="min-w-32 text-center text-xs text-foreground">
					{scenario.label}
					<span class="text-muted-foreground tabular-nums">
						· {index + 1}/{scenarios.length}
					</span>
				</span>
				<Button
					variant="ghost"
					size="icon-sm"
					aria-label="Next status"
					onclick={() => cycle(1)}
				>
					<PlayerTrackNext />
				</Button>
			</div>
			<label
				class="card flex items-center gap-2 rounded-full bg-linear-to-b from-surface-225 to-surface-200 px-3 py-1.5 text-xs text-muted-foreground dark:from-surface-800 dark:to-surface-925"
			>
				<span>Slices</span>
				<input
					type="range"
					min="2"
					max="100"
					step="1"
					bind:value={slices}
					class="w-36 accent-(--banner-info)"
				/>
				<span class="w-7 text-right text-foreground tabular-nums">{slices}</span>
			</label>
		</div>
	</div>
{/if}

<style>
	.banner-art {
		font-size: clamp(0.3rem, 0.85vw, 0.72rem);
		line-height: 1.2;
	}

	/*
	 * A light band sweeping through the caption; pure CSS because transition
	 * directives deadlock the async renderer.
	 */
	/* Solid foreground with a dimmer band sweeping through, so contrast never drops. */
	.status-shimmer {
		background: linear-gradient(
			100deg,
			var(--foreground) 42%,
			color-mix(in oklab, var(--foreground) 45%, transparent) 50%,
			var(--foreground) 58%
		);
		background-size: 200% 100%;
		background-clip: text;
		color: transparent;
		animation: status-shimmer 2.4s linear infinite;
	}

	@keyframes status-shimmer {
		from {
			background-position: 200% 0;
		}

		to {
			background-position: -200% 0;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.status-shimmer {
			animation: none;
			background: none;
			color: var(--foreground);
		}
	}
</style>
