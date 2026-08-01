<script lang="ts" effect>
	import { page } from "$app/state";
	import { Effect, Stream } from "effect";
	import { EngineMarkFor } from "$lib/engine/presentation";
	import {
		SessionDefaultsController,
		type SessionDefaultsState,
	} from "$lib/settings/session-defaults-controller";

	const defaults_controller = yield* SessionDefaultsController;
	const initial = yield* defaults_controller.Refresh.pipe(
		Effect.catch(() =>
			Effect.gen(function* () {
				return yield* defaults_controller.Current;
			}),
		),
	);
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

	type Anchor = { readonly hash: string; readonly label: string };
	type Item = {
		readonly anchors: ReadonlyArray<Anchor>;
		readonly href: string;
		readonly label: string;
	};

	const sections: ReadonlyArray<Item> = [
		{
			anchors: [
				{ hash: "compaction", label: "Compaction" },
				{ hash: "favorites", label: "Favorites" },
			],
			href: "/settings/models",
			label: "Models",
		},
		{
			anchors: [{ hash: "retention", label: "Retention" }],
			href: "/settings/threads",
			label: "Threads",
		},
	];
	const engines = $derived(runtime_catalog.manifest.harnesses.map((harness) => ({
		anchors: [
			{ hash: "account", label: "Account" },
			{ hash: "models", label: "Models" },
			{ hash: "permissions", label: "Permissions" },
		],
		href: `/settings/engines/${harness.id}`,
		label: harness.label,
		...EngineMarkFor(harness.id),
	})));

	const active_path = $derived(page.url.pathname.replace(/\/$/, ""));
	const active_hash = $derived(page.url.hash.slice(1));
</script>

{#snippet anchor_links(item: Item)}
	{#if active_path === item.href}
		<div class="flex flex-col gap-0.5 border-l border-border/60 pl-3 ml-1.5 mt-0.5 mb-1">
			{#each item.anchors as anchor (anchor.hash)}
				<a
					href={`${item.href}#${anchor.hash}`}
					class={active_hash === anchor.hash
						? "truncate rounded-sm py-0.5 text-xs text-foreground"
						: "truncate rounded-sm py-0.5 text-xs text-muted-foreground hover:text-foreground"}
				>
					{anchor.label}
				</a>
			{/each}
		</div>
	{/if}
{/snippet}

<nav aria-label="Settings sections" class="flex w-40 shrink-0 flex-col gap-0.5">
	{#each sections as item (item.href)}
		<a
			href={item.href}
			aria-current={active_path === item.href ? "page" : undefined}
			class={active_path === item.href
				? "truncate rounded-md px-2 py-1 text-sm font-medium text-foreground"
				: "truncate rounded-md px-2 py-1 text-sm text-muted-foreground hover:text-foreground"}
		>
			{item.label}
		</a>
		{@render anchor_links(item)}
	{/each}

	<span class="mt-4 mb-1 px-2 text-[0.7rem] font-medium tracking-wide text-muted-foreground uppercase">
		Engines
	</span>
	{#each engines as engine (engine.href)}
		{@const EngineIcon = engine.icon}
		<a
			href={engine.href}
			aria-current={active_path === engine.href ? "page" : undefined}
			class={active_path === engine.href
				? "flex items-center gap-2 truncate rounded-md px-2 py-1 text-sm font-medium text-foreground"
				: "flex items-center gap-2 truncate rounded-md px-2 py-1 text-sm text-muted-foreground hover:text-foreground"}
		>
			<EngineIcon
				class={engine.monochrome ? "size-3.5 shrink-0 dark:invert" : "size-3.5 shrink-0"}
			/>
			<span class="truncate">{engine.label}</span>
		</a>
		{@render anchor_links(engine)}
	{/each}
</nav>
