<script lang="ts" effect>
	import Bell from "@tabler/icons-svelte/icons/bell";
	import Messages from "@tabler/icons-svelte/icons/messages";
	import Palette from "@tabler/icons-svelte/icons/palette";
	import Sparkles from "@tabler/icons-svelte/icons/sparkles";
	import type { Component } from "svelte";
	import { page } from "$app/state";
	import { Effect, Stream } from "effect";
	import { EngineMarkFor } from "$lib/engine/presentation";
	import {
		SessionDefaultsController,
		type SessionDefaultsState,
	} from "$lib/settings/session-defaults-controller";
	import DropdownHoverSurface from "../dropdown-hover-surface.svelte";

	const defaults_controller = yield* SessionDefaultsController;
	const initial = yield* defaults_controller.Current;
	let defaults_state = $state.raw<SessionDefaultsState>(initial);
	const ApplyDefaults = (next: SessionDefaultsState) =>
		Effect.gen(function* () {
			defaults_state = next;
		});
	yield* defaults_controller.Changes.pipe(
		Stream.runForEach(ApplyDefaults),
		Effect.forkScoped,
	);
	const runtime_catalog = $derived(defaults_state.catalog);
	const disabled_engine_ids = $derived(
		new Set(defaults_state.defaults.disabled_engines ?? []),
	);

	type Anchor = { readonly hash: string; readonly label: string };
	type Item = {
		readonly anchors: ReadonlyArray<Anchor>;
		readonly href: string;
		readonly icon: Component;
		readonly label: string;
		readonly monochrome?: boolean;
	};

	const sections: ReadonlyArray<Item> = [
		{
			anchors: [
				{ hash: "compaction", label: "Compaction" },
				{ hash: "favorites", label: "Favorites" },
			],
			href: "/settings/models",
			icon: Sparkles,
			label: "Models",
		},
		{
			anchors: [
				{ hash: "retention", label: "Retention" },
				{ hash: "usage-recovery", label: "Usage recovery" },
				{ hash: "agents", label: "Agents" },
			],
			href: "/settings/threads",
			icon: Messages,
			label: "Threads",
		},
		{
			anchors: [
				{ hash: "typography", label: "Typography" },
				{ hash: "glass", label: "Glass" },
				{ hash: "reading", label: "Reading" },
			],
			href: "/settings/appearance",
			icon: Palette,
			label: "Appearance",
		},
		{
			anchors: [{ hash: "system", label: "System" }],
			href: "/settings/notifications",
			icon: Bell,
			label: "Notifications",
		},
	];
	const engines: ReadonlyArray<Item> = $derived(
		runtime_catalog.manifest.harnesses.map((harness) => ({
			anchors: [
				{ hash: "availability", label: "Availability" },
				{ hash: "installation", label: "Installation" },
				...(!disabled_engine_ids.has(harness.id)
					? [
							{ hash: "account", label: "Account" },
							{ hash: "models", label: "Models" },
						]
					: []),
			],
			href: `/settings/engines/${harness.id}`,
			label: harness.label,
			...EngineMarkFor(harness.id),
		})),
	);

	const active_path = $derived(page.url.pathname.replace(/\/$/, ""));
	const active_hash = $derived(page.url.hash.slice(1));
</script>

{#snippet group_label(label: string, spaced: boolean)}
	<span class={spaced
		? "mt-7 mb-1.5 hidden px-2 text-[0.625rem] font-medium tracking-[0.14em] text-muted-foreground uppercase md:block"
		: "mb-1.5 hidden px-2 text-[0.625rem] font-medium tracking-[0.14em] text-muted-foreground uppercase md:block"}
	>
		{label}
	</span>
{/snippet}

<!--
	Every hoverable row hands the pointer to the surface's traveling pill — the
	same contract the composer's model list uses — and stays `relative` so it
	paints above the pill. The active row wears the opaque gradient well instead.
-->
{#snippet nav_link(item: Item, move_hover: (event: Event) => void)}
	{@const active = active_path === item.href}
	{@const ItemIcon = item.icon}
	<a
		href={item.href}
		aria-current={active ? "page" : undefined}
		class={active
			? "card relative flex h-8 items-center gap-2 rounded-md bg-linear-to-b from-surface-225 to-surface-200 px-3 text-sm font-medium text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring/50 md:h-7 md:px-2 dark:from-surface-800 dark:to-surface-925"
			: "relative flex h-8 items-center gap-2 rounded-md px-3 text-sm text-muted-foreground transition-colors duration-(--duration-quick) outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 md:h-7 md:px-2 motion-reduce:transition-none"}
		onpointerenter={move_hover}
		onpointermove={move_hover}
		onfocusin={move_hover}
	>
		<ItemIcon class={item.monochrome === true ? "size-3.5 shrink-0 dark:invert" : "size-3.5 shrink-0"} />
		<span class="min-w-0 truncate">{item.label}</span>
	</a>
{/snippet}

<!--
	The rail's active tick sits exactly on the group border: `pl-3` keeps the
	link box 12px from the 1px border, so a marker 13px left of the box paints
	over the border line itself.
-->
{#snippet anchor_links(item: Item, move_hover: (event: Event) => void)}
	{#if active_path === item.href}
		<div class="my-1 ml-[0.9375rem] hidden flex-col border-l border-border/60 pl-3 md:flex">
			{#each item.anchors as anchor, index (anchor.hash)}
				<a
					href={`${item.href}#${anchor.hash}`}
					aria-current={active_hash === anchor.hash ? "location" : undefined}
					style={`animation-delay: ${index * 40}ms`}
					class={active_hash === anchor.hash
						? "relative flex h-6 items-center truncate rounded-md px-2 text-xs text-foreground outline-none animate-[settings-anchor-enter_var(--duration-quick)_var(--ease-smooth-out)_both] focus-visible:ring-2 focus-visible:ring-ring/50 before:absolute before:-left-[13px] before:top-1.5 before:bottom-1.5 before:w-px before:rounded-full before:bg-foreground"
						: "relative flex h-6 items-center truncate rounded-md px-2 text-xs text-muted-foreground outline-none animate-[settings-anchor-enter_var(--duration-quick)_var(--ease-smooth-out)_both] transition-colors duration-(--duration-quick) hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 motion-reduce:transition-none"}
					onpointerenter={move_hover}
					onpointermove={move_hover}
					onfocusin={move_hover}
				>
					{anchor.label}
				</a>
			{/each}
		</div>
	{/if}
{/snippet}

<nav
	aria-label="Settings sections"
	class="-mx-1 overflow-x-auto px-1 pb-1 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden md:mx-0 md:w-44 md:overflow-visible md:px-0 md:pb-0"
>
	<DropdownHoverSurface flat class="min-w-max md:min-w-0">
		{#snippet children({ move_hover })}
			<div class="flex items-center gap-1 md:block">
				{@render group_label("Artisan", false)}
				{#each sections as item (item.href)}
					{@render nav_link(item, move_hover)}
					{@render anchor_links(item, move_hover)}
				{/each}

				<span class="mx-1 h-4 w-px shrink-0 bg-border/60 md:hidden" aria-hidden="true"></span>
				{@render group_label("Engines", true)}
				{#each engines as engine (engine.href)}
					{@render nav_link(engine, move_hover)}
					{@render anchor_links(engine, move_hover)}
				{/each}
			</div>
		{/snippet}
	</DropdownHoverSurface>
</nav>
