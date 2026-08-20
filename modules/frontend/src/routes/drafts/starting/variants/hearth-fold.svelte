<script lang="ts">
	/**
	 * Hearth, where the fold is an actual fold.
	 *
	 * The first pass claimed the question "folds into a single line" and then
	 * swapped one block of markup for another, which is a cut, not a fold. Three
	 * things fix it, and none of them change the layout:
	 *
	 *   The chosen project's square is the same object before and after. It is
	 *   measured on the list, measured again on the header line, and played
	 *   between the two — so the eye follows one thing across the change and
	 *   never has to work out what became of what.
	 *
	 *   The rows that were not chosen draw in towards the one that was, instead
	 *   of vanishing where they stand. A list collapsing towards a point is the
	 *   picture the word "fold" was promising.
	 *
	 *   Both states sit in the same box at the same top edge, so nothing jumps.
	 *   The first pass centred the list and then centred a much shorter block,
	 *   which threw the whole page upwards at the moment of the choice.
	 *
	 * And the arrow keys work, because a list of seven things that can only be
	 * reached with a pointer is a list that has decided what your hands are
	 * doing.
	 */
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import FolderPlus from "@tabler/icons-svelte/icons/folder-plus";
	import { onDestroy, tick } from "svelte";
	import { DraftProjects, ThreadsFor } from "../mock";
	import Composer from "../pieces/composer.svelte";
	import Monogram from "../pieces/monogram.svelte";
	import RepoState from "../pieces/repo-state.svelte";
	import ThreadRow from "../pieces/thread-row.svelte";

	/**
	 * `folding` and `unfolding` are the window in which both layers exist. It is
	 * shorter than the square's flight on purpose: the layer being left should be
	 * gone well before the thing travelling out of it lands.
	 */
	type Phase = "folding" | "list" | "unfolding" | "work";

	const cross_ms = 200;
	const total = DraftProjects.length;

	let phase = $state<Phase>("list");
	/**
	 * How many times the list has been come back to. An entrance is for the
	 * first sight of something; replaying it on the way back would say the list
	 * had just been built, when what actually happened is that a square is on its
	 * way home to it. So the staggered arrival is spent once and the return is a
	 * plain dissolve underneath the square in flight.
	 */
	let returns = $state(0);
	let chosen_index = $state(0);
	let highlight_index = $state(0);
	let row_marks = $state<Array<HTMLElement | undefined>>([]);
	let header_mark = $state<HTMLElement | undefined>(undefined);
	let timer: ReturnType<typeof setTimeout> | undefined = undefined;

	const chosen = $derived(DraftProjects[chosen_index] ?? DraftProjects[0]);
	const threads = $derived(ThreadsFor(chosen.project_id));
	const show_list = $derived(phase !== "work");
	const show_work = $derived(phase !== "list");

	/**
	 * The layer being left steps out of flow, so the layer arriving is the one
	 * that decides how tall the page is and where its top edge falls — which is
	 * how both of them come to start at the same y without either reserving room
	 * for the other. It also sits underneath, because the thing you are going to
	 * be looking at should not be behind the thing you are done with.
	 */
	const LayerClass = (leaving: boolean) =>
		leaving ? "dz-leave absolute inset-x-0 top-0" : "relative z-10";

	/**
	 * Play a square in from where it used to be — the last two letters of FLIP,
	 * on the assumption the caller measured the first two.
	 *
	 * Here rather than in a shared module because it is only ever correct in one
	 * window: inside the handler, after `tick()` has committed the new layout and
	 * before the browser has painted it. WAAPI rather than a class, because the
	 * transform is computed from two measured rectangles and so cannot be written
	 * ahead of time; and nothing is left behind — no `fill`, no inline transform
	 * — so the square's resting style is still whatever the markup says.
	 */
	const FlipFrom = (node: HTMLElement, from: DOMRect) => {
		if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
		const to = node.getBoundingClientRect();
		if (to.width === 0 || from.width === 0) return;

		const shift_x = from.left - to.left;
		const shift_y = from.top - to.top;
		const scale = from.width / to.width;
		/** A move of under a pixel is not a move; animating it only costs a frame. */
		if (Math.abs(shift_x) < 1 && Math.abs(shift_y) < 1 && Math.abs(scale - 1) < 0.01) return;

		node.animate(
			[
				{
					transform: `translate(${shift_x}px, ${shift_y}px) scale(${scale})`,
					transformOrigin: "top left",
				},
				{ transform: "translate(0px, 0px) scale(1)", transformOrigin: "top left" },
			],
			/** Ionic's drawer curve: long in the tail, so the square reads as carried. */
			{ duration: 420, easing: "cubic-bezier(0.32, 0.72, 0, 1)" },
		);
	};

	const After = (run: () => void) => {
		if (timer !== undefined) clearTimeout(timer);
		timer = setTimeout(run, cross_ms);
	};

	onDestroy(() => {
		if (timer !== undefined) clearTimeout(timer);
	});

	const Fold = async (index: number) => {
		if (phase !== "list") return;
		const from = row_marks[index]?.getBoundingClientRect();
		chosen_index = index;
		highlight_index = index;
		phase = "folding";
		After(() => (phase = "work"));
		await tick();
		if (from !== undefined && header_mark !== undefined) FlipFrom(header_mark, from);
	};

	const Unfold = async () => {
		if (phase !== "work") return;
		const from = header_mark?.getBoundingClientRect();
		returns += 1;
		phase = "unfolding";
		After(() => (phase = "list"));
		await tick();
		const mark = row_marks[chosen_index];
		if (from !== undefined && mark !== undefined) FlipFrom(mark, from);
	};

	/**
	 * The gallery keeps the arrows that move between drafts and `h`; everything
	 * here is chosen from what it leaves alone. Nothing on this variant takes
	 * text, so there is no field to steal keys from.
	 */
	const OnKeydown = (event: KeyboardEvent) => {
		if (event.metaKey || event.ctrlKey || event.altKey) return;
		if (phase === "work") {
			if (event.key !== "Escape") return;
			void Unfold();
			event.preventDefault();
			return;
		}
		if (phase !== "list") return;
		if (event.key === "ArrowDown") highlight_index = (highlight_index + 1) % total;
		else if (event.key === "ArrowUp") highlight_index = (highlight_index - 1 + total) % total;
		else if (event.key === "Enter") void Fold(highlight_index);
		else return;
		event.preventDefault();
	};
</script>

<svelte:window onkeydown={OnKeydown} />

<!--
	Anchored near the top rather than centred. Centring was what threw the page
	upwards at the moment of the choice: a list of seven and a composer have
	nothing like the same height, and centring both means the shared content has
	to move for the difference.
-->
<div class="dz-vignette h-full overflow-y-auto px-8 pt-[10vh] pb-16">
	<div class="relative mx-auto w-full max-w-[42rem]">
		{#if show_list}
			<div class={`${LayerClass(phase === "folding")} ${returns > 0 ? "dz-quiet" : ""}`}>
				<div class={returns === 0 ? "dz-enter" : ""}>
					<h1 class="font-heading text-2xl text-foreground-extra">Where are we working?</h1>
					<p class="mt-1.5 text-sm text-muted-foreground">
						Every thread belongs to a project. Pick one to start.
					</p>
				</div>

				<div class="mt-7 flex flex-col gap-0.5">
					{#each DraftProjects as project, index (project.project_id)}
						<button
							class={`dz-row dz-press-row dz-focus dz-converge group flex items-center gap-3.5 rounded-xl px-3 py-3 text-left outline-none ${returns === 0 ? "dz-enter" : ""} ${index === highlight_index ? "dz-row-on" : ""} ${phase === "folding" && index !== chosen_index ? "dz-converged" : ""}`}
							onclick={() => Fold(index)}
							onpointerenter={() => (highlight_index = index)}
							style:--dz-converge={`${(chosen_index - index) * 5}px`}
							style:--dz-delay={`${30 + index * 32}ms`}
							type="button"
						>
							<span
								bind:this={row_marks[index]}
								class={`shrink-0 ${phase === "folding" && index === chosen_index ? "opacity-0" : ""}`}
							>
								<Monogram class="size-10 rounded-xl text-xs" {project} />
							</span>
							<span class="min-w-0 flex-1">
								<span class="block truncate text-[15px] text-foreground-extra">{project.name}</span>
								<span class="mt-0.5 flex items-center gap-2">
									<RepoState {project} />
								</span>
							</span>
							<span class="w-20 shrink-0 text-right text-xs text-muted-foreground">
								{project.last_used}
							</span>
							<ChevronRight
								class={`size-4 shrink-0 text-muted-foreground transition-opacity duration-150 ${index === highlight_index ? "opacity-100" : "opacity-0"}`}
							/>
						</button>
					{/each}

					<button
						class={`dz-row dz-press-row dz-focus mt-1 flex items-center gap-3.5 rounded-xl px-3 py-3 text-left text-sm text-muted-foreground outline-none ${returns === 0 ? "dz-enter" : ""}`}
						style:--dz-delay={`${30 + total * 32}ms`}
						type="button"
					>
						<span
							class="grid size-10 shrink-0 place-items-center rounded-xl border border-dashed border-border"
						>
							<FolderPlus class="size-4" />
						</span>
						Attach another folder
					</button>
				</div>

				<p
					class={`mt-4 px-3 font-mono text-[10px] text-surface-600 ${returns === 0 ? "dz-enter" : ""}`}
					style:--dz-delay={`${60 + total * 32}ms`}
				>
					↑ ↓ to move · ↵ to open
				</p>
			</div>
		{/if}

		{#if show_work}
			<div class={LayerClass(phase === "unfolding")}>
				<div class="dz-enter flex items-center gap-3" style:--dz-rise="6px">
					<span
						bind:this={header_mark}
						class={`shrink-0 ${phase === "unfolding" ? "opacity-0" : ""}`}
					>
						<Monogram class="size-8 rounded-lg text-[10px]" project={chosen} />
					</span>
					<span class="min-w-0">
						<span class="block truncate text-sm text-foreground-extra">{chosen.name}</span>
						<span class="block truncate font-mono text-xs text-muted-foreground">
							{chosen.path}
						</span>
					</span>
					<div class="flex-1"></div>
					<button
						class="dz-press dz-focus rounded-lg px-2.5 py-1.5 text-xs text-muted-foreground outline-none hover:bg-accent hover:text-foreground"
						onclick={() => Unfold()}
						type="button"
					>
						Change
						<span class="ml-1 font-mono text-[10px] text-surface-600">esc</span>
					</button>
				</div>

				<div class="dz-enter mt-5" style:--dz-delay="70ms">
					<Composer placeholder={`What should we do in ${chosen.name}?`} tall />
				</div>

				{#if threads.length > 0}
					<div class="mt-7">
						<p class="dz-enter px-2.5 text-xs text-muted-foreground" style:--dz-delay="140ms">
							Continue instead
						</p>
						<div class="mt-1 flex flex-col">
							{#each threads as thread, index (thread.thread_id)}
								<div class="dz-enter" style:--dz-delay={`${170 + index * 50}ms`}>
									<ThreadRow {thread} />
								</div>
							{/each}
						</div>
					</div>
				{/if}
			</div>
		{/if}
	</div>
</div>
