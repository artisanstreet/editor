<script lang="ts">
	import Settings from "@tabler/icons-svelte/icons/settings";
	import { EngineMarkClass, EngineMarkFor } from "$lib/engine/presentation";

	/**
	 * Design sandbox for the sidebar usage meters. Not linked from the app; open
	 * `/dev/usage` on the dev Forge to compare treatments side by side, then port
	 * the winner into `sidebar-identity.sv` and delete this route.
	 */

	type MockWindow = { readonly label: string; readonly percent: number; readonly resets: string };
	type MockEngine = {
		readonly id: string;
		readonly name: string;
		readonly session: ReadonlyArray<MockWindow>;
		readonly extended: ReadonlyArray<MockWindow>;
	};

	const engines: ReadonlyArray<MockEngine> = [
		{
			id: "codex",
			name: "Codex",
			session: [{ label: "5h", percent: 42, resets: "in 2h" }],
			extended: [
				{ label: "Weekly", percent: 8, resets: "in 7d" },
				{ label: "GPT-5.3-Codex-Spark", percent: 0, resets: "in 7d" },
			],
		},
		{
			id: "claude",
			name: "Claude",
			session: [{ label: "Session", percent: 16, resets: "in 3h" }],
			extended: [
				{ label: "Weekly", percent: 16, resets: "in 5d" },
				{ label: "Fable", percent: 27, resets: "in 5d" },
			],
		},
	];

	const variants = [
		{ id: "a", name: "A · Hairline rail", note: "Bar demoted to a 2px underline; the number leads." },
		{ id: "b", name: "B · Filled well", note: "The row itself is the tank — usage floods it." },
		{ id: "c", name: "C · Segmented ticks", note: "Instrument-like; reads quantised, not smooth." },
		{ id: "d", name: "D · Inline meter", note: "One line per window, fixed-width meter before the %." },
		{ id: "e", name: "E · Pips", note: "Ten dots, one per 10%. Playful, very light." },
		{ id: "f", name: "F · Rings", note: "No bars at all — a small conic dial per window." },
		{ id: "g", name: "G · Ledger", note: "Typography only. Number is the whole design." },
		{ id: "h", name: "H · Glow rail", note: "Inset well, gradient fill, accent bloom." },
	] as const;

	const clamp = (percent: number) => Math.min(100, Math.max(0, percent));
	const segments = 24;
	const pips = 10;
</script>

{#snippet mark(engine: MockEngine)}
	{@const resolved = EngineMarkFor(engine.id)}
	{@const Icon = resolved.icon}
	<div class="flex items-center gap-2">
		<Icon class={EngineMarkClass(resolved, "size-4")} />
		<span class="truncate text-xs font-medium text-foreground">{engine.name}</span>
	</div>
{/snippet}

<!-- A · Hairline rail -->
{#snippet variant_a(window: MockWindow)}
	<div class="flex flex-col gap-1">
		<div class="flex items-baseline justify-between gap-2">
			<span class="truncate text-xs text-muted-foreground">{window.label}</span>
			<span class="shrink-0 text-xs font-medium tabular-nums text-foreground/80">
				{window.percent}%
			</span>
		</div>
		<div class="h-px w-full bg-foreground/8">
			<div class="h-full bg-(--engine)" style={`width: ${clamp(window.percent)}%`}></div>
		</div>
	</div>
{/snippet}

<!-- B · Filled well -->
{#snippet variant_b(window: MockWindow)}
	<div class="inset-shadow relative overflow-hidden rounded-md bg-surface-850">
		<div
			class="absolute inset-y-0 left-0 bg-(--engine)/22 border-r border-(--engine)/60"
			style={`width: ${clamp(window.percent)}%`}
		></div>
		<div class="relative flex items-center justify-between gap-2 px-2 py-1.5">
			<span class="truncate text-xs text-foreground/80">{window.label}</span>
			<span class="shrink-0 text-xs tabular-nums text-muted-foreground">{window.percent}%</span>
		</div>
	</div>
{/snippet}

<!-- C · Segmented ticks -->
{#snippet variant_c(window: MockWindow)}
	<div class="flex flex-col gap-1.5">
		<div class="flex items-baseline justify-between gap-2">
			<span class="truncate text-xs text-muted-foreground">{window.label}</span>
			<span class="shrink-0 text-xs tabular-nums text-muted-foreground">{window.percent}%</span>
		</div>
		<div class="flex gap-[2px]">
			{#each { length: segments } as _, index (index)}
				{@const lit = (index + 1) / segments <= clamp(window.percent) / 100}
				<span
					class="h-2 flex-1 rounded-[1px] transition-colors"
					class:bg-foreground={false}
					style={`background: ${lit ? "var(--engine)" : "color-mix(in oklab, var(--foreground) 10%, transparent)"}`}
				></span>
			{/each}
		</div>
	</div>
{/snippet}

<!-- D · Inline meter -->
{#snippet variant_d(window: MockWindow)}
	<div class="flex items-center gap-2.5">
		<span class="min-w-0 flex-1 truncate text-xs text-muted-foreground">{window.label}</span>
		<span class="inset-shadow h-1.5 w-14 shrink-0 overflow-hidden rounded-full bg-surface-850">
			<span
				class="block h-full rounded-full bg-(--engine)"
				style={`width: ${clamp(window.percent)}%`}
			></span>
		</span>
		<span class="w-8 shrink-0 text-right text-xs tabular-nums text-foreground/80">
			{window.percent}%
		</span>
	</div>
{/snippet}

<!-- E · Pips -->
{#snippet variant_e(window: MockWindow)}
	<div class="flex items-center gap-2.5">
		<span class="min-w-0 flex-1 truncate text-xs text-muted-foreground">{window.label}</span>
		<span class="flex shrink-0 gap-1">
			{#each { length: pips } as _, index (index)}
				{@const lit = (index + 1) / pips <= clamp(window.percent) / 100}
				<span
					class="size-1.5 rounded-full"
					style={`background: ${lit ? "var(--engine)" : "color-mix(in oklab, var(--foreground) 12%, transparent)"}`}
				></span>
			{/each}
		</span>
		<span class="w-8 shrink-0 text-right text-xs tabular-nums text-foreground/80">
			{window.percent}%
		</span>
	</div>
{/snippet}

<!-- F · Rings -->
{#snippet variant_f(window: MockWindow)}
	<div class="flex items-center gap-2.5">
		<span
			class="size-4 shrink-0 rounded-full"
			style={`background: conic-gradient(var(--engine) ${clamp(window.percent) * 3.6}deg, color-mix(in oklab, var(--foreground) 12%, transparent) 0); mask: radial-gradient(circle, transparent 55%, #000 57%)`}
		></span>
		<span class="min-w-0 flex-1 truncate text-xs text-foreground/80">{window.label}</span>
		<span class="shrink-0 text-xs tabular-nums text-muted-foreground">
			{window.percent}% · {window.resets}
		</span>
	</div>
{/snippet}

<!-- G · Ledger -->
{#snippet variant_g(window: MockWindow)}
	<div class="flex items-baseline gap-2">
		<span class="w-10 shrink-0 text-sm font-medium tabular-nums text-(--engine)">
			{window.percent}%
		</span>
		<span class="min-w-0 flex-1 truncate text-xs text-foreground/70">{window.label}</span>
		<span class="shrink-0 text-[11px] tabular-nums text-muted-foreground">{window.resets}</span>
	</div>
{/snippet}

<!-- H · Glow rail -->
{#snippet variant_h(window: MockWindow)}
	<div class="flex flex-col gap-1">
		<div class="flex items-baseline justify-between gap-2">
			<span class="truncate text-xs text-muted-foreground">{window.label}</span>
			<span class="shrink-0 text-xs tabular-nums text-muted-foreground">
				{window.percent}% · {window.resets}
			</span>
		</div>
		<div class="inset-shadow h-1.5 w-full overflow-hidden rounded-full bg-surface-875">
			<div
				class="h-full rounded-full"
				style={`width: ${clamp(window.percent)}%; background: linear-gradient(90deg, color-mix(in oklab, var(--engine) 55%, transparent), var(--engine)); box-shadow: 0 0 8px color-mix(in oklab, var(--engine) 45%, transparent)`}
			></div>
		</div>
	</div>
{/snippet}

{#snippet row(id: string, window: MockWindow)}
	{#if id === "a"}{@render variant_a(window)}
	{:else if id === "b"}{@render variant_b(window)}
	{:else if id === "c"}{@render variant_c(window)}
	{:else if id === "d"}{@render variant_d(window)}
	{:else if id === "e"}{@render variant_e(window)}
	{:else if id === "f"}{@render variant_f(window)}
	{:else if id === "g"}{@render variant_g(window)}
	{:else}{@render variant_h(window)}{/if}
{/snippet}

<main class="min-h-svh bg-background p-10">
	<h1 class="mb-1 text-lg font-medium text-foreground">Usage meter treatments</h1>
	<p class="mb-8 text-sm text-muted-foreground">
		Same data, same panel. 5h limits lead each engine, separated from the longer windows.
	</p>

	<div class="flex flex-wrap gap-8">
		{#each variants as variant (variant.id)}
			<section class="flex w-64 flex-col gap-2">
				<div>
					<h2 class="text-sm font-medium text-foreground">{variant.name}</h2>
					<p class="text-xs text-muted-foreground">{variant.note}</p>
				</div>

				<div class="ring-foreground/10 rounded-2xl bg-popover p-1 shadow-2xl ring-1">
					<div
						class="flex items-center gap-2 rounded-xl px-2 py-1.5 text-sm text-popover-foreground"
					>
						<Settings class="size-4 shrink-0 text-muted-foreground" />
						Settings
					</div>
					<div class="bg-border/60 my-1 h-px"></div>

					<div class="flex flex-col gap-3 px-1 py-1">
						{#each engines as engine (engine.id)}
							<div
								class="flex flex-col gap-2 px-2 py-1"
								style={`--engine: ${EngineMarkFor(engine.id).accent}`}
							>
								{@render mark(engine)}
								<div class="flex flex-col gap-1.5">
									{#each engine.session as window (window.label)}
										{@render row(variant.id, window)}
									{/each}
								</div>
								<div class="flex flex-col gap-1.5">
									{#each engine.extended as window (window.label)}
										{@render row(variant.id, window)}
									{/each}
								</div>
							</div>
						{/each}
					</div>
				</div>
			</section>
		{/each}
	</div>
</main>
