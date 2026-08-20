<script lang="ts">
	/**
	 * The first run.
	 *
	 * The premise: every other variant assumes projects exist. On a fresh
	 * install none do, and that state deserves a designed screen rather than an
	 * empty list with a "+" in the corner — it is the only screen where the
	 * whole app is one decision wide.
	 *
	 * The second half is the part worth arguing about: Forge already knows what
	 * repositories are on the disk, so the empty state can offer them instead
	 * of opening a file dialog and hoping.
	 */
	import Folder from "@tabler/icons-svelte/icons/folder";
	import FolderPlus from "@tabler/icons-svelte/icons/folder-plus";
	import GitBranch from "@tabler/icons-svelte/icons/git-branch";
	import { DraftProjects } from "../mock";

	let dragging = $state(false);
	let attached = $state<ReadonlyArray<string>>([]);
	const found = DraftProjects.slice(1);
</script>

<div class="dz-vignette relative flex h-full items-center justify-center overflow-y-auto p-8">
	<div class="w-full max-w-[34rem]">
		<div
			class="dz-enter"
			ondragenter={(event) => {
				event.preventDefault();
				dragging = true;
			}}
			ondragleave={() => (dragging = false)}
			ondragover={(event) => event.preventDefault()}
			ondrop={(event) => {
				event.preventDefault();
				dragging = false;
			}}
			role="region"
		>
			<div
				class={`grid place-items-center rounded-3xl border border-dashed px-8 py-12 text-center transition-[background-color,border-color,transform] duration-200 ease-(--ease-smooth-out) motion-reduce:transition-none ${dragging ? "scale-[1.01] border-surface-400 bg-surface-875" : "border-border"}`}
			>
				<div
					class={`dz-tint grid size-14 place-items-center rounded-2xl transition-transform duration-200 ease-(--ease-smooth-out) motion-reduce:transition-none ${dragging ? "scale-110" : ""}`}
					style:--dz-hue="24"
				>
					<Folder class="size-6 text-foreground-extra" />
				</div>
				<h1 class="mt-5 font-heading text-xl text-foreground-extra">
					Artisan works inside a folder
				</h1>
				<p class="mt-2 max-w-sm text-sm text-balance text-muted-foreground">
					Attach the one you want to work in. Threads, sessions, and every change an engine makes
					stay scoped to it.
				</p>
				<button
					class="dz-press dz-focus mt-6 flex items-center gap-2 rounded-xl bg-surface-100 px-4 py-2.5 text-sm font-medium text-surface-950 outline-none"
					type="button"
				>
					<FolderPlus class="size-4" />
					Choose a folder…
				</button>
				<p class="mt-3 text-xs text-muted-foreground">or drop one anywhere on this panel</p>
			</div>
		</div>

		<div class="dz-enter mt-8" style:--dz-delay="120ms">
			<p class="px-1 text-xs text-muted-foreground">
				Repositories found near your home directory
			</p>
			<div class="mt-2 flex flex-col">
				{#each found as project, index (project.project_id)}
					{@const is_attached = attached.includes(project.project_id)}
					<div
						class="dz-row dz-enter flex items-center gap-3 rounded-lg px-2.5 py-2"
						style:--dz-delay={`${170 + index * 50}ms`}
					>
						<GitBranch class="size-4 shrink-0 text-muted-foreground" />
						<span class="min-w-0 flex-1">
							<span class="block truncate text-sm text-foreground">{project.name}</span>
							<span class="block truncate font-mono text-[11px] text-muted-foreground">
								{project.path}
							</span>
						</span>
						<button
							class={`dz-press dz-focus shrink-0 rounded-lg px-2.5 py-1.5 text-xs outline-none ${is_attached ? "text-muted-foreground" : "bg-surface-800 text-foreground hover:bg-surface-750"}`}
							disabled={is_attached}
							onclick={() => (attached = [...attached, project.project_id])}
							type="button"
						>
							{is_attached ? "Attached" : "Attach"}
						</button>
					</div>
				{/each}
			</div>
		</div>
	</div>
</div>
