<script lang="ts">
	/**
	 * The interface is a sentence.
	 *
	 * The premise: the whole state of the surface — who, where, on what branch —
	 * is one line of English, and the parts you can change are the words you can
	 * click. Nothing is labelled "Project:" because the sentence already says
	 * what the word is doing there.
	 *
	 * This is the variant that spends the most on tone and the least on
	 * function. It lives or dies on whether reading it once is faster than
	 * scanning a form, which is a real question and not a rhetorical one.
	 */
	import { DraftProjects, DraftThreads, type DraftProject } from "../mock";
	import Composer from "../pieces/composer.svelte";
	import ProjectMenu from "../pieces/project-menu.svelte";
	import ThreadRow from "../pieces/thread-row.svelte";

	let selected = $state<DraftProject>(DraftProjects[0]);
	const waiting = $derived(
		DraftThreads.filter(
			(thread) => thread.project_id === selected.project_id && thread.state === "asking",
		).length,
	);
</script>

<div class="dz-vignette relative flex h-full flex-col justify-center overflow-y-auto px-8 py-12">
	<div class="mx-auto w-full max-w-[46rem]">
		<h1
			class="dz-enter font-heading text-[2.6rem] leading-[1.15] tracking-tight text-balance text-muted-foreground"
		>
			<span class="dz-enter block text-foreground-extra" style:--dz-delay="0ms">Good evening.</span>
			<span class="dz-enter block" style:--dz-delay="90ms">
				You are working in
				<ProjectMenu
					class="inline-block"
					onselect={(project) => (selected = project)}
					origin="origin-top-left"
					placement="below"
					{selected}
				>
					{#snippet trigger({ open, project })}
						<span
							class={`-mx-1 rounded-lg px-1 text-foreground-extra decoration-2 underline-offset-[6px] transition-colors duration-150 ${open ? "bg-accent underline" : "underline decoration-dotted decoration-surface-600 hover:decoration-surface-400"}`}
						>
							{project.name}
						</span>
					{/snippet}
				</ProjectMenu>
			</span>
			<span class="dz-enter block" style:--dz-delay="170ms">
				on <span class="font-mono text-[0.85em] text-foreground">{selected.branch}</span>{#if selected.dirty > 0}, with
					<span class="text-foreground">{selected.dirty} files</span> changed{:else}, and nothing
					uncommitted{/if}.
			</span>
		</h1>

		<div class="dz-enter mt-10" style:--dz-delay="260ms">
			<Composer placeholder="What are we doing?" tall />
		</div>

		<div class="dz-enter mt-10 flex items-baseline gap-3" style:--dz-delay="340ms">
			<p class="text-sm text-muted-foreground">
				{#if waiting > 0}
					{waiting} {waiting === 1 ? "thread is" : "threads are"} waiting on you.
				{:else}
					Nothing is waiting on you.
				{/if}
			</p>
		</div>

		<div class="mt-2">
			{#each DraftThreads.filter((thread) => thread.project_id === selected.project_id) as thread, index (thread.thread_id)}
				<div class="dz-enter" style:--dz-delay={`${380 + index * 55}ms`}>
					<ThreadRow {thread} />
				</div>
			{/each}
		</div>
	</div>
</div>
