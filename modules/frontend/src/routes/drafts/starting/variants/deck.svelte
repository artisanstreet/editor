<script lang="ts">
	/**
	 * Projects as objects.
	 *
	 * The premise: with a handful of projects, a list is an inventory but a deck
	 * is a place. The one you are in is a card you are holding; the rest are
	 * stacked behind it, visible enough to remember they exist and quiet enough
	 * not to compete.
	 *
	 * Every card is mounted the whole time and only its transform changes, so
	 * flicking through the stack is one interruptible motion rather than a
	 * sequence of mounts. Grabbing the deck mid-slide retargets it.
	 */
	import ChevronLeft from "@tabler/icons-svelte/icons/chevron-left";
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import { DraftProjects, ThreadsFor } from "../mock";
	import Composer from "../pieces/composer.svelte";
	import Monogram from "../pieces/monogram.svelte";
	import RepoState from "../pieces/repo-state.svelte";
	import Spark from "../pieces/spark.svelte";

	let active_index = $state(0);
	const total = DraftProjects.length;
	const active = $derived(DraftProjects[active_index] ?? DraftProjects[0]);
	/** Depth in the stack: 0 is the card in hand, and the order wraps. */
	const depth_of = (index: number) => (index - active_index + total) % total;
	const Advance = (step: number) => {
		active_index = (active_index + step + total) % total;
	};
</script>

<div class="dz-vignette relative flex h-full flex-col items-center justify-center overflow-hidden p-8">
	<div class="dz-enter relative h-[19rem] w-full max-w-[27rem]" style:perspective="1200px">
		{#each DraftProjects as project, index (project.project_id)}
			{@const depth = depth_of(index)}
			<button
				aria-hidden={depth !== 0}
				class="card-lg absolute inset-x-0 top-0 rounded-2xl border border-border bg-linear-to-b from-surface-875 to-surface-900 p-6 text-left transition-[transform,opacity] duration-(--panel-open-dur) ease-(--ease-smooth-out) motion-reduce:transition-none"
				onclick={() => (depth === 0 ? undefined : Advance(depth))}
				style:opacity={depth > 3 ? 0 : 1 - depth * 0.16}
				style:pointer-events={depth > 3 ? "none" : "auto"}
				style:transform={`translateY(${-depth * 16}px) scale(${1 - depth * 0.045})`}
				style:z-index={total - depth}
				tabindex={depth === 0 ? 0 : -1}
				type="button"
			>
				<div class="flex items-start gap-4">
					<Monogram class="size-12 rounded-2xl text-sm" {project} />
					<div class="min-w-0 flex-1">
						<p class="truncate font-heading text-lg text-foreground-extra">{project.name}</p>
						<p class="mt-0.5 truncate font-mono text-xs text-muted-foreground">{project.path}</p>
					</div>
					<span class="shrink-0 text-xs text-muted-foreground">{project.last_used}</span>
				</div>

				<div class="mt-6 h-14 opacity-70">
					<Spark class="h-full" values={project.activity} />
				</div>

				<div class="mt-5 flex items-center justify-between gap-4 border-t border-border pt-4">
					<RepoState {project} />
					<span class="shrink-0 text-xs text-muted-foreground">
						{ThreadsFor(project.project_id).length} open · {project.threads} total
					</span>
				</div>
			</button>
		{/each}
	</div>

	<div class="dz-enter mt-2 flex items-center gap-3" style:--dz-delay="120ms">
		<button
			aria-label="Previous project"
			class="dz-press dz-focus dz-well grid size-8 place-items-center rounded-lg text-foreground outline-none"
			onclick={() => Advance(-1)}
			type="button"
		>
			<ChevronLeft class="size-4" />
		</button>
		<span class="w-24 text-center text-xs tabular-nums text-muted-foreground">
			{active_index + 1} of {total}
		</span>
		<button
			aria-label="Next project"
			class="dz-press dz-focus dz-well grid size-8 place-items-center rounded-lg text-foreground outline-none"
			onclick={() => Advance(1)}
			type="button"
		>
			<ChevronRight class="size-4" />
		</button>
	</div>

	<div class="dz-enter mt-8 w-full max-w-[34rem]" style:--dz-delay="180ms">
		<Composer placeholder={`Start something in ${active.name}`} />
	</div>
</div>
