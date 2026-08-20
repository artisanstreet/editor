<script lang="ts">
	/**
	 * Projects as a segmented control.
	 *
	 * The premise: the set is small and stable, so it can be laid out flat —
	 * every project visible, switching in one click with no menu to open. The
	 * page below stays exactly the landing page it is today.
	 *
	 * The colour change is done by clipping, not by timing two colours against
	 * each other. Two identical rows are stacked, the top one styled as though
	 * every segment were active, and only the active segment's rectangle is
	 * left unclipped. Sliding that rectangle moves the pill and repaints the
	 * label in one motion, which no pair of colour transitions can match.
	 */
	import { DraftProjects, DraftThreads } from "../mock";
	import Composer from "../pieces/composer.svelte";
	import Monogram from "../pieces/monogram.svelte";
	import RepoState from "../pieces/repo-state.svelte";
	import ThreadRow from "../pieces/thread-row.svelte";

	/** Fixed, so the clip can be expressed in the same unit the track is built from. */
	const segment_rem = 8;
	let active_index = $state(0);
	const active = $derived(DraftProjects[active_index] ?? DraftProjects[0]);
	const clip = $derived(
		`inset(0 calc(100% - ${(active_index + 1) * segment_rem}rem) 0 ${active_index * segment_rem}rem round 9999px)`,
	);
	const threads = $derived(
		DraftThreads.filter((thread) => thread.project_id === active.project_id),
	);
</script>

<div class="dz-vignette relative flex h-full flex-col items-center overflow-y-auto px-8 py-10">
	<div class="flex w-full max-w-[42rem] flex-1 flex-col">
		<div class="dz-enter no-scrollbar overflow-x-auto rounded-full">
			<div
				class="relative w-max rounded-full bg-surface-900 p-1"
				style:width={`${DraftProjects.length * segment_rem}rem`}
			>
				<div class="relative grid" style:grid-template-columns={`repeat(${DraftProjects.length}, ${segment_rem}rem)`}>
					{#each DraftProjects as project, index (project.project_id)}
						<button
							class="dz-focus truncate rounded-full px-3 py-1.5 text-center text-[13px] text-muted-foreground outline-none transition-colors duration-200 hover:text-foreground"
							onclick={() => (active_index = index)}
							type="button"
						>
							{project.name}
						</button>
					{/each}

					<div
						aria-hidden="true"
						class="pointer-events-none absolute inset-0 grid rounded-full bg-surface-200 transition-[clip-path] duration-(--duration-fast) ease-(--ease-smooth-out) motion-reduce:transition-none"
						style:clip-path={clip}
						style:grid-template-columns={`repeat(${DraftProjects.length}, ${segment_rem}rem)`}
					>
						{#each DraftProjects as project (project.project_id)}
							<span class="truncate px-3 py-1.5 text-center text-[13px] font-medium text-surface-950">
								{project.name}
							</span>
						{/each}
					</div>
				</div>
			</div>
		</div>

		{#key active.project_id}
			<div class="dz-enter mt-8 flex items-center gap-3" style:--dz-rise="6px">
				<Monogram class="size-9 rounded-lg text-[11px]" project={active} />
				<div class="min-w-0">
					<p class="truncate font-mono text-xs text-muted-foreground">{active.path}</p>
					<div class="mt-1"><RepoState project={active} /></div>
				</div>
			</div>

			<div class="mt-4 min-h-0 flex-1">
				{#each threads as thread, index (thread.thread_id)}
					<div class="dz-enter" style:--dz-delay={`${60 + index * 50}ms`}>
						<ThreadRow {thread} />
					</div>
				{:else}
					<p class="dz-enter px-2.5 py-6 text-sm text-muted-foreground" style:--dz-delay="60ms">
						No threads in {active.name} yet — the field below starts the first one.
					</p>
				{/each}
			</div>
		{/key}

		<div class="dz-enter mt-6" style:--dz-delay="120ms">
			<Composer placeholder={`New thread in ${active.name}`} />
		</div>
	</div>
</div>
