<script lang="ts">
	/**
	 * One field for everything.
	 *
	 * The premise: picking a project and finding a thread and writing a message
	 * are the same gesture — you type, you filter, you press Enter. There is no
	 * mode switch and no second surface, and because the field is already
	 * focused on arrival, the fastest path to work is to start typing.
	 *
	 * No entrance animation on the field itself. This is the surface someone
	 * hits a hundred times a day; a hundred 200ms curtains is four minutes a
	 * year spent watching a box arrive.
	 */
	import CornerDownLeft from "@tabler/icons-svelte/icons/corner-down-left";
	import Search from "@tabler/icons-svelte/icons/search";
	import X from "@tabler/icons-svelte/icons/x";
	import {
		DraftProjects,
		DraftThreads,
		EnginePresentation,
		ThreadStateTone,
		type DraftProject,
	} from "../mock";
	import Monogram from "../pieces/monogram.svelte";
	import RepoState from "../pieces/repo-state.svelte";

	let query = $state("");
	let pinned = $state<DraftProject | undefined>(undefined);

	const matches = (text: string) => text.toLowerCase().includes(query.trim().toLowerCase());
	const projects = $derived(
		pinned === undefined ? DraftProjects.filter((project) => matches(project.name)) : [],
	);
	const threads = $derived(
		DraftThreads.filter(
			(thread) =>
				matches(thread.title) &&
				(pinned === undefined || thread.project_id === pinned.project_id),
		).slice(0, 5),
	);
	const project_named = (project_id: string) =>
		DraftProjects.find((project) => project.project_id === project_id)?.name ?? "";
</script>

<div class="dz-vignette relative flex h-full justify-center overflow-y-auto px-8 pt-[18vh] pb-8">
	<div class="w-full max-w-[38rem]">
		<div
			class="card radius-surface overflow-hidden border border-border bg-linear-to-b from-surface-875 to-surface-900"
			style:--radius-gap="0.5rem"
			style:--radius-surface="0.875rem"
		>
			<div class="flex items-center gap-2.5 px-3.5 py-3">
				{#if pinned === undefined}
					<Search class="size-4 shrink-0 text-muted-foreground" />
				{:else}
					<button
						class="dz-press dz-focus dz-well flex shrink-0 items-center gap-1.5 rounded-md py-1 pr-1.5 pl-1 text-xs text-foreground outline-none"
						onclick={() => (pinned = undefined)}
						type="button"
					>
						<Monogram class="size-4 rounded-[4px] text-[7px]" project={pinned} />
						{pinned.name}
						<X class="size-3 text-muted-foreground" />
					</button>
				{/if}
				<!-- svelte-ignore a11y_autofocus -->
				<input
					autofocus
					bind:value={query}
					class="min-w-0 flex-1 bg-transparent text-[15px] text-foreground placeholder:text-muted-foreground focus:outline-none"
					placeholder={pinned === undefined
						? "Search projects and threads, or start typing"
						: `Message ${pinned.name}…`}
					type="text"
				/>
				{#if pinned !== undefined && query.length > 0}
					<span class="flex shrink-0 items-center gap-1 text-[11px] text-muted-foreground">
						<CornerDownLeft class="size-3" /> send
					</span>
				{/if}
			</div>

			{#if projects.length > 0 || threads.length > 0}
				<div class="border-t border-border p-1.5">
					{#if projects.length > 0}
						<p class="px-2 pt-1 pb-1.5 text-[11px] tracking-wide text-muted-foreground">
							Projects
						</p>
						{#each projects as project (project.project_id)}
							<button
								class="dz-row dz-press-row dz-focus flex w-full items-center gap-2.5 rounded-lg px-2 py-1.5 text-left outline-none"
								onclick={() => {
									pinned = project;
									query = "";
								}}
								type="button"
							>
								<Monogram class="size-6 rounded-md text-[9px]" {project} />
								<span class="min-w-0 flex-1 truncate text-sm text-foreground">{project.name}</span>
								<RepoState class="shrink-0" {project} />
							</button>
						{/each}
					{/if}

					{#if threads.length > 0}
						<p class="px-2 pt-2.5 pb-1.5 text-[11px] tracking-wide text-muted-foreground">
							Threads
						</p>
						{#each threads as thread (thread.thread_id)}
							<button
								class="dz-row dz-press-row dz-focus flex w-full items-center gap-2.5 rounded-lg px-2 py-1.5 text-left outline-none"
								type="button"
							>
								<span
									class="state-dot size-1.5 shrink-0 rounded-full"
									style:--state-dot-tone={ThreadStateTone[thread.state]}
								></span>
								<span
									aria-hidden="true"
									class="shrink-0 text-[10px]"
									style:color={EnginePresentation[thread.engine].tone}
								>
									{EnginePresentation[thread.engine].glyph}
								</span>
								<span class="min-w-0 flex-1 truncate text-sm text-foreground">{thread.title}</span>
								{#if pinned === undefined}
									<span class="shrink-0 font-mono text-[11px] text-muted-foreground">
										{project_named(thread.project_id)}
									</span>
								{/if}
							</button>
						{/each}
					{/if}
				</div>
			{/if}
		</div>

		<p class="mt-3 px-1 text-[11px] text-muted-foreground">
			A project has to be chosen before the first send — pinning one turns the field into the
			composer.
		</p>
	</div>
</div>
