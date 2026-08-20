<script lang="ts">
	/**
	 * Rail, quieter.
	 *
	 * The first pass had the right idea and the wrong manners. Switching project
	 * is something you do tens of times a day, and it was staged like an arrival:
	 * the whole panel was keyed on the project id, so every click tore the pane
	 * down and played a staggered entrance for the header, then the count, then
	 * each thread in turn. Correct for a page you land on, a tax on a page you
	 * live in. Here the pane never remounts and the swap is a 120ms dissolve —
	 * long enough not to teleport, short enough never to be waited on.
	 *
	 * The rest is housekeeping the first pass got away with:
	 *
	 *   The indicator was positioned by multiplying the index by a hardcoded
	 *   52px. It now reads the tile it is pointing at, so it cannot drift when
	 *   the tile size, the gap, or the padding changes.
	 *
	 *   A rail of unlabelled squares is a memory test. Each tile names itself on
	 *   hover, out of its own right edge, and carries the shortcut that selects
	 *   it — which is how anyone ever learns a shortcut.
	 *
	 *   The sparkline is gone. Twenty-eight bars of decoration were the largest
	 *   object in a header whose job was to say where you are.
	 *
	 *   Tiles show a dot when something is running or waiting in that project,
	 *   which is the argument for the rail being permanent in the first place.
	 */
	import FolderPlus from "@tabler/icons-svelte/icons/folder-plus";
	import { DraftProjects, ThreadStateTone, ThreadsFor, type DraftProject } from "../mock";
	import Composer from "../pieces/composer.svelte";
	import Monogram from "../pieces/monogram.svelte";
	import RepoState from "../pieces/repo-state.svelte";
	import ThreadRow from "../pieces/thread-row.svelte";

	let active_index = $state(0);
	let tiles = $state<Array<HTMLElement | undefined>>([]);
	let indicator_y = $state(0);

	const active = $derived(DraftProjects[active_index] ?? DraftProjects[0]);
	const threads = $derived(ThreadsFor(active.project_id));

	/** The tile is the authority on where the indicator goes; the rail is not. */
	$effect(() => {
		const tile = tiles[active_index];
		if (tile !== undefined) indicator_y = tile.offsetTop;
	});

	/** Nothing to say about a project where nothing is happening. */
	const LiveTone = (project: DraftProject): string | undefined => {
		const waiting = ThreadsFor(project.project_id).find((thread) => thread.state !== "idle");
		return waiting === undefined ? undefined : ThreadStateTone[waiting.state];
	};

	/**
	 * The gallery hands over anything held with a modifier, so the workspace
	 * shortcut everyone already has muscle memory for is free to take.
	 */
	const OnKeydown = (event: KeyboardEvent) => {
		if (!(event.metaKey || event.ctrlKey) || event.altKey) return;
		const position = Number.parseInt(event.key, 10);
		if (!Number.isInteger(position) || position < 1 || position > DraftProjects.length) return;
		active_index = position - 1;
		event.preventDefault();
	};
</script>

<svelte:window onkeydown={OnKeydown} />

<div class="dz-vignette relative flex h-full overflow-hidden">
	<nav
		aria-label="Projects"
		class="relative flex w-[4.5rem] shrink-0 flex-col items-center gap-3 border-r border-border bg-surface-925 py-5"
	>
		<div
			class="pointer-events-none absolute top-0 left-0 h-10 w-0.5 rounded-r-full bg-foreground-extra transition-transform duration-(--duration-quick) ease-(--ease-smooth-out)"
			style:transform={`translateY(${indicator_y}px)`}
		></div>

		{#each DraftProjects as project, index (project.project_id)}
			{@const tone = LiveTone(project)}
			<button
				aria-current={index === active_index}
				bind:this={tiles[index]}
				class="dz-press dz-focus dz-enter-x dz-flyout-host relative rounded-xl outline-none"
				onclick={() => (active_index = index)}
				style:--dz-delay={`${index * 40}ms`}
				type="button"
			>
				<Monogram
					class={`size-10 rounded-xl text-xs transition-opacity duration-200 ${index === active_index ? "opacity-100" : "opacity-45 hover:opacity-85"}`}
					{project}
				/>

				{#if tone !== undefined}
					<span
						class="state-dot absolute -top-0.5 -right-0.5 size-2 rounded-full ring-2 ring-surface-925"
						style:--state-dot-tone={tone}
					></span>
				{/if}

				<!--
					Two elements, because two things want the transform: the outer one
					centres the label on the tile and never moves, the inner one is the
					only thing the reveal is allowed to touch.
				-->
				<span
					class="pointer-events-none absolute top-1/2 left-[calc(100%+0.625rem)] z-20 -translate-y-1/2"
				>
					<span
						class="dz-flyout card flex items-center gap-2 rounded-lg border border-border bg-surface-900/95 px-2.5 py-1.5 whitespace-nowrap backdrop-blur-md"
					>
						<span class="text-xs text-foreground-extra">{project.name}</span>
						<span class="font-mono text-[10px] text-surface-600">⌘{index + 1}</span>
					</span>
				</span>
			</button>
		{/each}

		<button
			class="dz-press dz-focus dz-enter-x dz-flyout-host relative grid size-10 place-items-center rounded-xl border border-dashed border-border text-muted-foreground outline-none hover:text-foreground"
			style:--dz-delay={`${DraftProjects.length * 40}ms`}
			type="button"
		>
			<FolderPlus class="size-4" />
			<span
				class="pointer-events-none absolute top-1/2 left-[calc(100%+0.625rem)] z-20 -translate-y-1/2"
			>
				<span
					class="dz-flyout card block rounded-lg border border-border bg-surface-900/95 px-2.5 py-1.5 text-xs whitespace-nowrap text-foreground-extra backdrop-blur-md"
				>
					Attach a folder
				</span>
			</span>
		</button>
	</nav>

	<div class="flex min-w-0 flex-1 flex-col overflow-hidden">
		<!--
			Keyed on the project so the content dissolves rather than cutting, and
			keyed no higher than it needs to be: the composer below sits outside,
			because a field that survives the switch is a field you can still be
			halfway through typing into.
		-->
		{#key active.project_id}
			<div class="dz-quiet flex min-h-0 flex-1 flex-col">
				<header class="shrink-0 px-8 pt-8">
					<h1 class="truncate font-heading text-xl text-foreground-extra">{active.name}</h1>
					<p class="mt-1 truncate font-mono text-xs text-muted-foreground">{active.path}</p>
					<div class="mt-2"><RepoState project={active} /></div>
				</header>

				<div class="min-h-0 flex-1 overflow-y-auto px-6 pt-6">
					<p class="px-2.5 pb-1 text-xs text-muted-foreground">
						{threads.length} open {threads.length === 1 ? "thread" : "threads"}
					</p>
					{#each threads as thread (thread.thread_id)}
						<ThreadRow {thread} />
					{:else}
						<p class="px-2.5 py-8 text-sm text-muted-foreground">Nothing running here yet.</p>
					{/each}
				</div>
			</div>
		{/key}

		<div class="shrink-0 px-6 pt-4 pb-6">
			<div class="mx-auto w-full max-w-[46rem]">
				<Composer placeholder={`New thread in ${active.name}`} />
			</div>
		</div>
	</div>
</div>
