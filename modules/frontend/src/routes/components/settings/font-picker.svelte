<script lang="ts" effect>
	import Selector from "@tabler/icons-svelte/icons/selector";
	import { Effect, Result } from "effect";

	import {
		bundled_font_families,
		type TypographyFamily,
		type TypographyRole,
	} from "$lib/appearance/typography";
	import { BrowserTypography } from "$lib/browser/typography";
	import { Button } from "$lib/components/ui/button";
	import {
		Command,
		CommandGroup,
		CommandInput,
		CommandItem,
		CommandList,
	} from "$lib/components/ui/command";
	import { Popover, PopoverContent, PopoverTrigger } from "$lib/components/ui/popover";

	type DiscoveryState =
		| { readonly phase: "idle" }
		| { readonly phase: "loading" }
		| { readonly phase: "ready" }
		| { readonly phase: "unavailable" }
		| { readonly phase: "denied" }
		| { readonly phase: "failed" };

	let {
		family,
		onselect,
		role,
	}: {
		family: TypographyFamily;
		onselect: (family: TypographyFamily) => Effect.Effect<void>;
		role: TypographyRole;
	} = $props();

	const browser_typography = yield* BrowserTypography;
	const local_fonts_supported = yield* browser_typography.LocalFontsSupported;
	const max_visible_fonts = 80;

	let open = $state(false);
	let search = $state("");
	let local_families = $state.raw<ReadonlyArray<TypographyFamily>>([]);
	let discovery = $state.raw<DiscoveryState>(
		local_fonts_supported ? { phase: "idle" } : { phase: "unavailable" },
	);

	const role_label = $derived(role === "text" ? "Text" : "Code");
	const search_key = $derived(search.trim().toLowerCase());
	const family_key = $derived(family.toLowerCase());
	const family_is_bundled = $derived(
		bundled_font_families.some((candidate) => candidate.toLowerCase() === family_key),
	);
	const custom_family_matches = $derived(
		!family_is_bundled && family_key.includes(search_key),
	);
	const matching_included_families = $derived(
		bundled_font_families.filter((candidate) => candidate.toLowerCase().includes(search_key)),
	);
	const matching_local_families = $derived.by(() => {
		const excluded_keys = new Set([
			family_key,
			...bundled_font_families.map((candidate) => candidate.toLowerCase()),
		]);
		return local_families.filter(
			(candidate) =>
				!excluded_keys.has(candidate.toLowerCase()) &&
				candidate.toLowerCase().includes(search_key),
		);
	});
	const visible_local_families = $derived(matching_local_families.slice(0, max_visible_fonts));
	const hidden_local_count = $derived(
		Math.max(0, matching_local_families.length - visible_local_families.length),
	);

	const DiscoverFonts = Effect.gen(function* () {
		if (discovery.phase === "loading" || discovery.phase === "ready") return;
		if (!local_fonts_supported) {
			discovery = { phase: "unavailable" };
			return;
		}

		discovery = { phase: "loading" };
		const discovered = yield* browser_typography.DiscoverLocalFonts.pipe(Effect.result);
		if (Result.isSuccess(discovered)) {
			local_families = discovered.success;
			discovery = { phase: "ready" };
			return;
		}

		switch (discovered.failure._tag) {
			case "LocalFontsUnavailable":
				discovery = { phase: "unavailable" };
				break;
			case "LocalFontsDenied":
				discovery = { phase: "denied" };
				break;
			case "LocalFontsInvalid":
			case "LocalFontsFailure":
				discovery = { phase: "failed" };
				break;
		}
	});

	const DiscoverOnOpen = Effect.gen(function* () {
		if (discovery.phase === "idle") yield* DiscoverFonts;
	});

	const SelectFamily = (next_family: TypographyFamily) =>
		Effect.gen(function* () {
			yield* onselect(next_family);
			open = false;
			search = "";
		});
</script>

<Popover bind:open>
	<PopoverTrigger
		aria-label={`Choose ${role_label.toLowerCase()} font. Current font: ${family}`}
		class="card flex h-8 w-full max-w-full shrink-0 items-center gap-2 rounded-md bg-linear-to-b from-surface-225 to-surface-200 px-2.5 text-left text-xs text-foreground outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:ring-inset sm:w-48 sm:max-w-[42vw] dark:from-surface-800 dark:to-surface-925"
		onclick={yield* DiscoverOnOpen}
	>
		<span
			class:font-sans={role === "text"}
			class:font-mono={role === "code"}
			class="min-w-0 flex-1 truncate"
		>
			{family}
		</span>
		<Selector class="pointer-events-none size-3.5 shrink-0 text-muted-foreground" />
	</PopoverTrigger>

	<PopoverContent
		align="end"
		side="bottom"
		sideOffset={8}
		class="w-[min(22rem,calc(100vw-2rem))] gap-0 p-1"
	>
		<Command shouldFilter={false} class="rounded-xl bg-transparent p-0">
			<CommandInput bind:value={search} placeholder="Search fonts…" aria-label={`Search ${role_label.toLowerCase()} fonts`} />
			<CommandList class="max-h-64">
				{#if custom_family_matches}
					<CommandGroup heading="Current">
						<CommandItem
							value={family}
							aria-current="true"
							data-checked={true}
							onSelect={yield* SelectFamily(family)}
						>
							<span class="truncate">{family}</span>
						</CommandItem>
					</CommandGroup>
				{/if}

				{#if matching_included_families.length > 0}
					<CommandGroup heading="Included with Artisan">
						{#each matching_included_families as candidate (candidate.toLowerCase())}
							<CommandItem
								value={candidate}
								aria-current={candidate.toLowerCase() === family_key ? "true" : undefined}
								data-checked={candidate.toLowerCase() === family.toLowerCase()}
								onSelect={yield* SelectFamily(candidate)}
							>
								<span class="truncate">{candidate}</span>
							</CommandItem>
						{/each}
					</CommandGroup>
				{/if}

				{#if visible_local_families.length > 0}
					<CommandGroup heading="On this device">
						{#each visible_local_families as candidate (candidate.toLowerCase())}
							<CommandItem
								value={candidate}
								aria-current={candidate.toLowerCase() === family_key ? "true" : undefined}
								data-checked={candidate.toLowerCase() === family.toLowerCase()}
								onSelect={yield* SelectFamily(candidate)}
							>
								<span class="truncate">{candidate}</span>
							</CommandItem>
						{/each}
					</CommandGroup>
				{/if}

				{#if !custom_family_matches && matching_included_families.length === 0 && visible_local_families.length === 0 && discovery.phase !== "loading"}
					<div class="px-3 py-7 text-center text-xs text-muted-foreground">
						No fonts match “{search}”.
					</div>
				{/if}
			</CommandList>

			<div
				class="flex min-h-9 items-center justify-between gap-3 border-t border-border/50 px-3 py-2 text-[0.7rem] text-muted-foreground"
				aria-live="polite"
			>
				{#if discovery.phase === "loading"}
					<span class="flex items-center gap-2">
						<span class="size-1.5 animate-pulse rounded-full bg-current"></span>
						Reading font names from this device…
					</span>
				{:else if discovery.phase === "unavailable"}
					<span>Local font discovery is unavailable here.</span>
				{:else if discovery.phase === "denied"}
					<span>Local font access wasn't allowed.</span>
					<Button variant="ghost" size="xs" onclick={yield* DiscoverFonts}>Try again</Button>
				{:else if discovery.phase === "failed"}
					<span>Artisan couldn't read local fonts.</span>
					<Button variant="ghost" size="xs" onclick={yield* DiscoverFonts}>Try again</Button>
				{:else if discovery.phase === "ready" && hidden_local_count > 0}
					<span>{hidden_local_count} more — refine the search to narrow the list.</span>
				{:else if discovery.phase === "ready" && local_families.length === 0}
					<span>No additional local font families found.</span>
				{:else if discovery.phase === "ready"}
					<span>{local_families.length} local {local_families.length === 1 ? "family" : "families"}</span>
				{:else}
					<span>Open the picker to read local font names.</span>
				{/if}
			</div>
		</Command>
	</PopoverContent>
</Popover>
