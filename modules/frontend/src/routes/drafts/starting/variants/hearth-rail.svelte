<script lang="ts">
	/**
	 * Hearth and Rail are the same surface at two moments.
	 *
	 * Both drafts survived the first pass, and holding them side by side the
	 * argument between them dissolves: Hearth is what the rail looks like before
	 * you have answered it, and the rail is what the list becomes once you have.
	 * They disagree about when, not about what. So this one refuses to choose —
	 * the list you pick from *is* the rail you then live in, and choosing is the
	 * animation between the two.
	 *
	 * Which means every project square has to be the same object throughout. One
	 * `<nav>`, one set of buttons, two sets of classes: the list is a wide column
	 * in the middle of the page, the rail is a narrow one against the left edge,
	 * and every square is measured before the swap and played in from where it
	 * used to be. Labels are positioned absolutely so their disappearance is a
	 * fade rather than a reflow the squares would have to chase.
	 *
	 * The cost is that the first choice is unskippable, which was Hearth's whole
	 * point. The gain is that it is only ever made once, which was Rail's.
	 */
	import FolderPlus from "@tabler/icons-svelte/icons/folder-plus";
	import { tick } from "svelte";
	import { DraftProjects, ThreadsFor } from "../mock";
	import Composer from "../pieces/composer.svelte";
	import Monogram from "../pieces/monogram.svelte";
	import RepoState from "../pieces/repo-state.svelte";
	import ThreadRow from "../pieces/thread-row.svelte";

	let picking = $state(true);
	let active_index = $state(0);
	/** The squares that travel. */
	let marks = $state<Array<HTMLElement | undefined>>([]);
	/** The buttons the indicator points at, which are not the same elements. */
	let tiles = $state<Array<HTMLElement | undefined>>([]);
	let indicator_y = $state(0);

	const active = $derived(DraftProjects[active_index] ?? DraftProjects[0]);
	const threads = $derived(ThreadsFor(active.project_id));

	$effect(() => {
		const tile = tiles[active_index];
		if (tile !== undefined && !picking) indicator_y = tile.offsetTop;
	});

	/**
	 * Everything that moves is measured, then moved, then told where it came
	 * from — in that order, in one pass, inside the handler, after `tick()` has
	 * committed the new layout and before the browser has painted it. That window
	 * is the whole technique, which is why this is here and not in a module that
	 * would have to be able to suspend in the middle of it.
	 */
	const Relayout = async (mutate: () => void) => {
		const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
		const before = marks.map((node) => node?.getBoundingClientRect());
		mutate();
		await tick();
		if (reduced) return;

		marks.forEach((node, index) => {
			const from = before[index];
			if (node === undefined || from === undefined) return;
			const to = node.getBoundingClientRect();
			if (to.width === 0 || from.width === 0) return;

			const shift_x = from.left - to.left;
			const shift_y = from.top - to.top;
			/** A move of under a pixel is not a move; animating it only costs a frame. */
			if (Math.abs(shift_x) < 1 && Math.abs(shift_y) < 1) return;

			node.animate(
				[
					{ transform: `translate(${shift_x}px, ${shift_y}px)` },
					{ transform: "translate(0px, 0px)" },
				],
				/** Ionic's drawer curve: long in the tail, so the squares read as carried. */
				{ duration: 460, easing: "cubic-bezier(0.32, 0.72, 0, 1)" },
			);
		});
	};

	const Choose = (index: number) =>
		Relayout(() => {
			active_index = index;
			picking = false;
		});

	const Reopen = () => Relayout(() => (picking = true));
</script>

<div class={`dz-vignette flex h-full overflow-hidden ${picking ? "items-center justify-center" : ""}`}>
	<nav
		aria-label="Projects"
		class={`relative flex flex-col transition-colors duration-200 ${
			picking
				? "w-full max-w-[36rem] gap-0.5 border-r border-transparent px-2"
				: "w-[4.5rem] shrink-0 items-center gap-3 border-r border-border bg-surface-925 py-5"
		}`}
	>
		<!--
			Out of flow on purpose. The question sits above the list without being
			part of it, so the nav's centre — and therefore every square's resting
			position — is unchanged by whether it is there.
		-->
		<div
			class={`pointer-events-none absolute bottom-full left-0 w-full pb-7 pl-3 transition-opacity duration-200 ${picking ? "opacity-100" : "opacity-0"}`}
		>
			<h1 class="font-heading text-2xl text-foreground-extra">Where are we working?</h1>
			<p class="mt-1.5 text-sm text-muted-foreground">
				Pick one. It stays on the left afterwards.
			</p>
		</div>

		<div
			class={`pointer-events-none absolute top-0 left-0 h-10 w-0.5 rounded-r-full bg-foreground-extra transition-[transform,opacity] duration-(--duration-quick) ease-(--ease-smooth-out) ${picking ? "opacity-0" : "opacity-100"}`}
			style:transform={`translateY(${indicator_y}px)`}
		></div>

		{#each DraftProjects as project, index (project.project_id)}
			<button
				aria-current={!picking && index === active_index}
				bind:this={tiles[index]}
				class={`dz-enter dz-focus group relative flex items-center outline-none ${
					picking
						? "dz-row dz-press-row w-full gap-3.5 rounded-xl px-3 py-2.5 text-left"
						: "dz-press rounded-xl"
				}`}
				onclick={() => Choose(index)}
				style:--dz-delay={`${index * 40}ms`}
				type="button"
			>
				<span bind:this={marks[index]} class="shrink-0">
					<Monogram
						class={`size-10 rounded-xl text-xs transition-opacity duration-200 ${picking || index === active_index ? "opacity-100" : "opacity-45 group-hover:opacity-85"}`}
						{project}
					/>
				</span>

				<span
					class={`pointer-events-none absolute top-1/2 left-[3.5rem] flex -translate-y-1/2 flex-col items-start transition-opacity duration-150 ${picking ? "opacity-100" : "opacity-0"}`}
				>
					<span class="truncate text-[15px] text-foreground-extra">{project.name}</span>
					<RepoState class="mt-0.5" {project} />
				</span>

				<span
					class={`pointer-events-none absolute top-1/2 right-3 -translate-y-1/2 text-xs whitespace-nowrap text-muted-foreground transition-opacity duration-150 ${picking ? "opacity-100" : "opacity-0"}`}
				>
					{project.last_used}
				</span>
			</button>
		{/each}

		<button
			class={`dz-enter dz-focus relative flex items-center text-muted-foreground outline-none hover:text-foreground ${
				picking
					? "dz-row dz-press-row mt-1 w-full gap-3.5 rounded-xl px-3 py-2.5 text-left text-sm"
					: "dz-press rounded-xl"
			}`}
			style:--dz-delay={`${DraftProjects.length * 40}ms`}
			type="button"
		>
			<span bind:this={marks[DraftProjects.length]} class="shrink-0">
				<span
					class="grid size-10 place-items-center rounded-xl border border-dashed border-border"
				>
					<FolderPlus class="size-4" />
				</span>
			</span>
			<span
				class={`pointer-events-none absolute top-1/2 left-[3.5rem] -translate-y-1/2 whitespace-nowrap transition-opacity duration-150 ${picking ? "opacity-100" : "opacity-0"}`}
			>
				Attach another folder
			</span>
		</button>
	</nav>

	{#if !picking}
		<div class="dz-enter flex min-w-0 flex-1 flex-col overflow-hidden" style:--dz-delay="150ms">
			<header class="flex shrink-0 items-start gap-4 px-8 pt-8">
				<div class="min-w-0 flex-1">
					<h1 class="truncate font-heading text-xl text-foreground-extra">{active.name}</h1>
					<p class="mt-1 truncate font-mono text-xs text-muted-foreground">{active.path}</p>
					<div class="mt-2"><RepoState project={active} /></div>
				</div>
				<button
					class="dz-press dz-focus shrink-0 rounded-lg px-2.5 py-1.5 text-xs text-muted-foreground outline-none hover:bg-accent hover:text-foreground"
					onclick={() => Reopen()}
					type="button"
				>
					All projects
				</button>
			</header>

			{#key active.project_id}
				<div class="dz-quiet min-h-0 flex-1 overflow-y-auto px-6 pt-6">
					<p class="px-2.5 pb-1 text-xs text-muted-foreground">
						{threads.length} open {threads.length === 1 ? "thread" : "threads"}
					</p>
					{#each threads as thread (thread.thread_id)}
						<ThreadRow {thread} />
					{:else}
						<p class="px-2.5 py-8 text-sm text-muted-foreground">Nothing running here yet.</p>
					{/each}
				</div>
			{/key}

			<div class="shrink-0 px-6 pt-4 pb-6">
				<div class="mx-auto w-full max-w-[46rem]">
					<Composer placeholder={`New thread in ${active.name}`} />
				</div>
			</div>
		</div>
	{/if}
</div>
