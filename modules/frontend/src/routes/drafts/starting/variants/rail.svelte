<script lang="ts">
	/**
	 * The project is ambient.
	 *
	 * The premise: you do not pick a project per thread, you are *in* one — the
	 * way you are in a Slack workspace or a VS Code window. A permanent rail on
	 * the left holds every project, the page under it is always scoped to the
	 * one lit up, and switching costs one click from anywhere.
	 *
	 * The indicator is a single element that slides. An indicator that unmounts
	 * and remounts per tab cannot tween, and this one moves far more often than
	 * it appears.
	 */
	import FolderPlus from "@tabler/icons-svelte/icons/folder-plus";
	import { DraftProjects, ThreadsFor } from "../mock";
	import Composer from "../pieces/composer.svelte";
	import Monogram from "../pieces/monogram.svelte";
	import RepoState from "../pieces/repo-state.svelte";
	import Spark from "../pieces/spark.svelte";
	import ThreadRow from "../pieces/thread-row.svelte";

	/** One rail slot: a 40px tile plus the 12px of air below it. */
	const slot_pitch = 52;
	let active_index = $state(0);
	const active = $derived(DraftProjects[active_index] ?? DraftProjects[0]);
	const threads = $derived(ThreadsFor(active.project_id));
</script>

<div class="dz-vignette relative flex h-full overflow-hidden">
	<nav
		aria-label="Projects"
		class="relative flex w-[4.5rem] shrink-0 flex-col items-center gap-3 border-r border-border bg-surface-925 py-5"
	>
		<div
			class="pointer-events-none absolute top-5 left-0 h-10 w-0.5 rounded-r-full bg-foreground-extra transition-transform duration-(--duration-fast) ease-(--ease-smooth-out) motion-reduce:transition-none"
			style:transform={`translateY(${active_index * slot_pitch}px)`}
		></div>

		{#each DraftProjects as project, index (project.project_id)}
			<button
				aria-current={index === active_index}
				class="dz-press dz-focus dz-enter-x relative rounded-xl outline-none"
				onclick={() => (active_index = index)}
				style:--dz-delay={`${index * 40}ms`}
				title={project.name}
				type="button"
			>
				<Monogram
					class={`size-10 rounded-xl text-xs transition-opacity duration-200 ${index === active_index ? "opacity-100" : "opacity-45 hover:opacity-80"}`}
					{project}
				/>
			</button>
		{/each}

		<button
			class="dz-press dz-focus dz-enter-x grid size-10 place-items-center rounded-xl border border-dashed border-border text-muted-foreground outline-none hover:text-foreground"
			style:--dz-delay={`${DraftProjects.length * 40}ms`}
			title="Attach a folder"
			type="button"
		>
			<FolderPlus class="size-4" />
		</button>
	</nav>

	<div class="flex min-w-0 flex-1 flex-col overflow-hidden">
		{#key active.project_id}
			<header class="dz-enter shrink-0 px-8 pt-8" style:--dz-rise="6px">
				<div class="flex items-end justify-between gap-6">
					<div class="min-w-0">
						<h1 class="truncate font-heading text-xl text-foreground-extra">{active.name}</h1>
						<p class="mt-1 truncate font-mono text-xs text-muted-foreground">{active.path}</p>
						<div class="mt-2"><RepoState project={active} /></div>
					</div>
					<div class="hidden h-10 w-56 shrink-0 opacity-60 md:block">
						<Spark class="h-full" values={active.activity} />
					</div>
				</div>
			</header>

			<div class="min-h-0 flex-1 overflow-y-auto px-6 pt-6">
				<p class="dz-enter px-2.5 pb-1 text-xs text-muted-foreground" style:--dz-delay="60ms">
					{threads.length} open {threads.length === 1 ? "thread" : "threads"}
				</p>
				{#each threads as thread, index (thread.thread_id)}
					<div class="dz-enter" style:--dz-delay={`${90 + index * 50}ms`}>
						<ThreadRow {thread} />
					</div>
				{:else}
					<p class="dz-enter px-2.5 py-8 text-sm text-muted-foreground" style:--dz-delay="90ms">
						Nothing running here yet.
					</p>
				{/each}
			</div>
		{/key}

		<div class="shrink-0 px-6 pt-4 pb-6">
			<div class="mx-auto w-full max-w-[46rem]">
				<Composer placeholder={`New thread in ${active.name}`} />
			</div>
		</div>
	</div>
</div>
