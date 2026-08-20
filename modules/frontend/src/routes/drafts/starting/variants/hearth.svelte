<script lang="ts">
	/**
	 * Project first.
	 *
	 * The premise: a thread cannot exist without a project, so the surface asks
	 * for the project before it offers the field. Nothing else is on screen
	 * until that is answered, and once it is, the question folds into a single
	 * line and gives the whole page to the work.
	 *
	 * The cost is a step. The gain is that the step is never skipped by
	 * accident, which is what "projects[0]" has been doing.
	 */
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import FolderPlus from "@tabler/icons-svelte/icons/folder-plus";
	import { DraftProjects, ThreadsFor, type DraftProject } from "../mock";
	import Composer from "../pieces/composer.svelte";
	import Monogram from "../pieces/monogram.svelte";
	import RepoState from "../pieces/repo-state.svelte";
	import Spark from "../pieces/spark.svelte";
	import ThreadRow from "../pieces/thread-row.svelte";

	let selected = $state<DraftProject | undefined>(undefined);
	const threads = $derived(selected === undefined ? [] : ThreadsFor(selected.project_id));
</script>

<div class="dz-vignette relative grid h-full place-items-center overflow-y-auto p-8">
	<div class="w-full max-w-[42rem]">
		{#if selected === undefined}
			<div class="dz-enter" style:--dz-delay="0ms">
				<h1 class="font-heading text-2xl text-foreground-extra">Where are we working?</h1>
				<p class="mt-1.5 text-sm text-muted-foreground">
					Every thread belongs to a project. Pick one to start.
				</p>
			</div>

			<div class="mt-7 flex flex-col gap-0.5">
				{#each DraftProjects as project, index (project.project_id)}
					<button
						class="dz-row dz-press-row dz-focus dz-enter group flex items-center gap-3.5 rounded-xl px-3 py-3 text-left outline-none"
						onclick={() => (selected = project)}
						style:--dz-delay={`${40 + index * 45}ms`}
						type="button"
					>
						<Monogram class="size-10 rounded-xl text-xs" {project} />
						<span class="min-w-0 flex-1">
							<span class="block truncate text-[15px] text-foreground-extra">{project.name}</span>
							<span class="mt-0.5 flex items-center gap-2">
								<RepoState {project} />
							</span>
						</span>
						<span class="hidden h-7 w-24 shrink-0 opacity-50 sm:block">
							<Spark class="h-full" values={project.activity} />
						</span>
						<span class="w-20 shrink-0 text-right text-xs text-muted-foreground">
							{project.last_used}
						</span>
						<ChevronRight
							class="size-4 shrink-0 text-muted-foreground opacity-0 transition-opacity duration-150 group-hover:opacity-100"
						/>
					</button>
				{/each}

				<button
					class="dz-row dz-press-row dz-focus dz-enter mt-1 flex items-center gap-3.5 rounded-xl px-3 py-3 text-left text-sm text-muted-foreground outline-none"
					style:--dz-delay={`${40 + DraftProjects.length * 45}ms`}
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
		{:else}
			<div class="dz-enter flex items-center gap-3" style:--dz-rise="6px">
				<Monogram class="size-8 rounded-lg text-[10px]" project={selected} />
				<span class="min-w-0">
					<span class="block truncate text-sm text-foreground-extra">{selected.name}</span>
					<span class="block truncate font-mono text-xs text-muted-foreground">
						{selected.path}
					</span>
				</span>
				<div class="flex-1"></div>
				<button
					class="dz-press dz-focus rounded-lg px-2.5 py-1.5 text-xs text-muted-foreground outline-none hover:bg-accent hover:text-foreground"
					onclick={() => (selected = undefined)}
					type="button"
				>
					Change
				</button>
			</div>

			<div class="dz-enter mt-5" style:--dz-delay="70ms">
				<Composer placeholder={`What should we do in ${selected.name}?`} tall />
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
		{/if}
	</div>
</div>
