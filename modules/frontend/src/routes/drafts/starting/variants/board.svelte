<script lang="ts">
	/**
	 * The picker is the dashboard.
	 *
	 * The premise: the moment before you choose a project is the one moment you
	 * genuinely want to know what every project is doing — what is dirty, what
	 * is running, what is waiting on an answer. So the chooser is not a menu
	 * over the page, it *is* the page, and the right pane answers "what is in
	 * there?" before the click rather than after it.
	 *
	 * Hover previews, click commits. Reading is free; only the click changes
	 * where the composer will send.
	 */
	import { DraftProjects, ThreadsFor, type DraftProject } from "../mock";
	import Composer from "../pieces/composer.svelte";
	import Monogram from "../pieces/monogram.svelte";
	import RepoState from "../pieces/repo-state.svelte";
	import Spark from "../pieces/spark.svelte";
	import ThreadRow from "../pieces/thread-row.svelte";

	let selected = $state<DraftProject>(DraftProjects[0]);
	let preview = $state<DraftProject | undefined>(undefined);
	const shown = $derived(preview ?? selected);
	const threads = $derived(ThreadsFor(shown.project_id));
	const running = (project: DraftProject) =>
		ThreadsFor(project.project_id).filter((thread) => thread.state === "running").length;
</script>

<div class="dz-vignette relative flex h-full overflow-hidden">
	<aside
		class="flex w-[23rem] shrink-0 flex-col border-r border-border bg-surface-925"
		onpointerleave={() => (preview = undefined)}
	>
		<p class="shrink-0 px-5 pt-7 pb-3 text-xs tracking-wide text-muted-foreground">
			Projects · {DraftProjects.length}
		</p>
		<div class="min-h-0 flex-1 overflow-y-auto px-2 pb-4">
			{#each DraftProjects as project, index (project.project_id)}
				<button
					class={`dz-row dz-press-row dz-focus dz-enter-x relative flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left outline-none ${project.project_id === selected.project_id ? "bg-surface-875" : ""}`}
					onclick={() => (selected = project)}
					onpointerenter={() => (preview = project)}
					style:--dz-delay={`${index * 40}ms`}
					type="button"
				>
					{#if project.project_id === selected.project_id}
						<span class="absolute inset-y-2 left-0 w-0.5 rounded-r-full bg-foreground-extra"></span>
					{/if}
					<Monogram class="size-8 rounded-lg text-[10px]" {project} />
					<span class="min-w-0 flex-1">
						<span class="flex items-center gap-2">
							<span class="min-w-0 truncate text-sm text-foreground-extra">{project.name}</span>
							{#if running(project) > 0}
								<span
									class="state-dot size-1.5 shrink-0 rounded-full"
									style:--state-dot-tone="var(--banner-success)"
								></span>
							{/if}
						</span>
						<RepoState class="mt-0.5" {project} />
					</span>
					<span class="shrink-0 text-[11px] text-muted-foreground">{project.last_used}</span>
				</button>
			{/each}
		</div>
	</aside>

	<div class="flex min-w-0 flex-1 flex-col">
		{#key shown.project_id}
			<header class="dz-enter shrink-0 border-b border-border px-8 py-6" style:--dz-rise="4px">
				<div class="flex items-start justify-between gap-8">
					<div class="min-w-0">
						<h1 class="truncate font-heading text-xl text-foreground-extra">{shown.name}</h1>
						<p class="mt-1 truncate font-mono text-xs text-muted-foreground">{shown.path}</p>
					</div>
					<div class="h-10 w-64 shrink-0 opacity-60">
						<Spark class="h-full" values={shown.activity} />
					</div>
				</div>
				<dl class="mt-5 flex gap-8 text-xs">
					<div>
						<dt class="text-muted-foreground">Branch</dt>
						<dd class="mt-0.5 font-mono text-foreground">{shown.branch}</dd>
					</div>
					<div>
						<dt class="text-muted-foreground">Uncommitted</dt>
						<dd class="mt-0.5 tabular-nums text-foreground">{shown.dirty} files</dd>
					</div>
					<div>
						<dt class="text-muted-foreground">Threads</dt>
						<dd class="mt-0.5 tabular-nums text-foreground">{shown.threads}</dd>
					</div>
					<div>
						<dt class="text-muted-foreground">Last used</dt>
						<dd class="mt-0.5 text-foreground">{shown.last_used}</dd>
					</div>
				</dl>
			</header>

			<div class="min-h-0 flex-1 overflow-y-auto px-6 py-5">
				{#each threads as thread, index (thread.thread_id)}
					<div class="dz-enter" style:--dz-delay={`${index * 45}ms`}>
						<ThreadRow {thread} />
					</div>
				{:else}
					<p class="dz-enter px-2.5 py-6 text-sm text-muted-foreground">
						Nothing open in {shown.name}.
					</p>
				{/each}
			</div>
		{/key}

		<div class="shrink-0 border-t border-border px-6 py-5">
			<div class="mx-auto w-full max-w-[46rem]">
				<Composer placeholder={`New thread in ${selected.name}`} />
				{#if preview !== undefined && preview.project_id !== selected.project_id}
					<p class="mt-2 px-1 text-[11px] text-muted-foreground">
						Previewing {preview.name} · sends to {selected.name} until you click
					</p>
				{/if}
			</div>
		</div>
	</div>
</div>
