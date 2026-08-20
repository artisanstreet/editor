<script lang="ts">
	/**
	 * Everything at once.
	 *
	 * The premise: the landing surface is the only screen with no work on it, so
	 * it can afford to be the status board — the project you are in, blown up;
	 * the others, as tiles; what is open, beside them. Choosing is a side effect
	 * of looking.
	 *
	 * Tiles are unequal on purpose. A grid of identical cards makes every
	 * project look equally important, and they are not: one of them is where
	 * you have spent the last month.
	 */
	import FolderPlus from "@tabler/icons-svelte/icons/folder-plus";
	import { DraftProjects, DraftThreads, ThreadsFor, type DraftProject } from "../mock";
	import Calendar from "../pieces/calendar.svelte";
	import Composer from "../pieces/composer.svelte";
	import Monogram from "../pieces/monogram.svelte";
	import RepoState from "../pieces/repo-state.svelte";
	import Spark from "../pieces/spark.svelte";
	import ThreadRow from "../pieces/thread-row.svelte";

	let selected = $state<DraftProject>(DraftProjects[0]);
	const others = $derived(
		DraftProjects.filter((project) => project.project_id !== selected.project_id).slice(0, 3),
	);
	const open_threads = $derived(DraftThreads.slice(0, 5));
</script>

<div class="dz-vignette relative h-full overflow-y-auto p-6">
	<div class="mx-auto flex min-h-full w-full max-w-[64rem] flex-col justify-center">
		<div class="grid grid-cols-4 gap-3">
			<!-- The project you are in, at the size that says so. -->
			<section
				class="dz-enter card col-span-2 row-span-2 flex flex-col rounded-2xl border border-border bg-linear-to-b from-surface-875 to-surface-900 p-6"
			>
				<div class="flex items-start gap-4">
					<Monogram class="size-12 rounded-2xl text-sm" project={selected} />
					<div class="min-w-0 flex-1">
						<p class="truncate font-heading text-lg text-foreground-extra">{selected.name}</p>
						<p class="mt-0.5 truncate font-mono text-xs text-muted-foreground">{selected.path}</p>
					</div>
				</div>
				<div class="mt-6 flex-1 overflow-hidden">
					<Calendar class="h-full" values={selected.activity.concat(selected.activity, selected.activity)} />
				</div>
				<div class="mt-6 flex items-center justify-between gap-4 border-t border-border pt-4">
					<RepoState project={selected} />
					<span class="shrink-0 text-xs text-muted-foreground">{selected.threads} threads</span>
				</div>
			</section>

			<!-- What is open, everywhere. Switching projects does not empty this. -->
			<section
				class="dz-enter card col-span-2 row-span-2 flex min-h-0 flex-col rounded-2xl border border-border bg-surface-925 p-3"
				style:--dz-delay="70ms"
			>
				<p class="px-2.5 pt-1.5 pb-2 text-xs tracking-wide text-muted-foreground">Open threads</p>
				<div class="min-h-0 flex-1 overflow-y-auto">
					{#each open_threads as thread, index (thread.thread_id)}
						<div class="dz-enter" style:--dz-delay={`${120 + index * 45}ms`}>
							<ThreadRow {thread} />
						</div>
					{/each}
				</div>
			</section>

			{#each others as project, index (project.project_id)}
				<button
					class="dz-row dz-press dz-focus dz-enter card flex flex-col items-start gap-2.5 rounded-2xl border border-border bg-surface-925 p-4 text-left outline-none"
					onclick={() => (selected = project)}
					style:--dz-delay={`${150 + index * 60}ms`}
					type="button"
				>
					<Monogram class="size-8 rounded-lg text-[10px]" {project} />
					<span class="w-full min-w-0">
						<span class="block truncate text-sm text-foreground-extra">{project.name}</span>
						<span class="mt-0.5 block truncate text-[11px] text-muted-foreground">
							{project.last_used}
						</span>
					</span>
					<span class="h-5 w-full opacity-50">
						<Spark class="h-full" values={project.activity} />
					</span>
				</button>
			{/each}

			<button
				class="dz-row dz-press dz-focus dz-enter flex flex-col items-start justify-center gap-2.5 rounded-2xl border border-dashed border-border p-4 text-left text-muted-foreground outline-none hover:text-foreground"
				style:--dz-delay="330ms"
				type="button"
			>
				<FolderPlus class="size-5" />
				<span class="text-sm">Attach a folder</span>
				<span class="text-[11px] opacity-70">{DraftProjects.length} attached</span>
			</button>
		</div>

		<div class="dz-enter mt-3" style:--dz-delay="400ms">
			<Composer placeholder={`New thread in ${selected.name}`} />
		</div>

		<p class="dz-enter mt-2 px-1 text-[11px] text-muted-foreground" style:--dz-delay="450ms">
			{ThreadsFor(selected.project_id).length} of these are in {selected.name}.
		</p>
	</div>
</div>
