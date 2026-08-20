<script lang="ts">
	/**
	 * For the hands that never leave the keyboard.
	 *
	 * The premise: this audience already has a mental model for "choose a
	 * directory, then run something in it", and it is a terminal. So the surface
	 * borrows the terminal's grammar — a fixed table, an index per row, a prompt
	 * at the bottom — and spends its whole design budget on alignment and
	 * legibility rather than on chrome.
	 *
	 * Arrow keys and j/k move the selection. Nothing animates on a keystroke:
	 * this is the surface someone drives fifty times a day, and fifty small
	 * curtains a day is the difference between fast and merely quick.
	 */
	import { DraftProjects, ThreadsFor } from "../mock";

	let cursor = $state(0);
	const active = $derived(DraftProjects[cursor] ?? DraftProjects[0]);

	const OnKeydown = (event: KeyboardEvent) => {
		const step = event.key === "ArrowDown" || event.key === "j"
			? 1
			: event.key === "ArrowUp" || event.key === "k"
				? -1
				: 0;
		if (step === 0) return;
		event.preventDefault();
		cursor = (cursor + step + DraftProjects.length) % DraftProjects.length;
	};
</script>

<svelte:window onkeydown={OnKeydown} />

<div class="relative flex h-full flex-col overflow-hidden bg-surface-975 p-8 font-mono text-[13px]">
	<div class="mx-auto flex w-full max-w-[56rem] flex-1 flex-col overflow-hidden">
		<p class="dz-enter shrink-0 text-muted-foreground">
			<span class="text-foreground-extra">artisan</span> ▸ open
			<span class="text-surface-500">— select a project</span>
		</p>

		<div class="dz-enter mt-6 min-h-0 flex-1 overflow-y-auto" style:--dz-delay="50ms">
			<table class="w-full border-collapse text-left tabular-nums">
				<thead>
					<tr class="text-[11px] tracking-wide text-surface-500 uppercase">
						<th class="w-8 py-1.5 pr-3 font-normal"></th>
						<th class="py-1.5 pr-6 font-normal">project</th>
						<th class="py-1.5 pr-6 font-normal">branch</th>
						<th class="w-20 py-1.5 pr-6 text-right font-normal">dirty</th>
						<th class="w-20 py-1.5 pr-6 text-right font-normal">open</th>
						<th class="w-32 py-1.5 text-right font-normal">last</th>
					</tr>
				</thead>
				<tbody>
					{#each DraftProjects as project, index (project.project_id)}
						<tr
							class={`dz-enter cursor-default ${index === cursor ? "bg-surface-875 text-foreground-extra" : "text-surface-400 hover:bg-surface-925"}`}
							onpointerenter={() => (cursor = index)}
							style:--dz-delay={`${80 + index * 30}ms`}
						>
							<td class="py-1.5 pr-3 text-surface-600">
								{index === cursor ? "▸" : index + 1}
							</td>
							<td class="max-w-0 truncate py-1.5 pr-6">
								{project.name}
								<span class="text-surface-600">{project.path.replace("~", "")}</span>
							</td>
							<td class="py-1.5 pr-6 text-surface-500">{project.branch}</td>
							<td
								class={`py-1.5 pr-6 text-right ${project.dirty > 0 ? "text-warning" : "text-surface-600"}`}
							>
								{project.dirty === 0 ? "—" : `+${project.dirty}`}
							</td>
							<td class="py-1.5 pr-6 text-right text-surface-500">
								{ThreadsFor(project.project_id).length || "—"}
							</td>
							<td class="py-1.5 text-right text-surface-600">{project.last_used}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>

		<div class="dz-enter mt-6 shrink-0 border-t border-surface-875 pt-5" style:--dz-delay="200ms">
			<div class="flex items-baseline gap-2">
				<span class="shrink-0 text-surface-500">
					{active.name}
					<span class="text-surface-700">▸</span>
				</span>
				<span class="min-w-0 flex-1 text-surface-600">
					describe a change<span class="dz-caret ml-0.5 inline-block w-[7px] bg-surface-300">&nbsp;</span>
				</span>
			</div>
			<p class="mt-5 flex flex-wrap gap-x-5 gap-y-1 text-[11px] text-surface-600">
				<span><span class="text-surface-400">↑↓ / jk</span> select</span>
				<span><span class="text-surface-400">⏎</span> start a thread</span>
				<span><span class="text-surface-400">1–{DraftProjects.length}</span> jump</span>
				<span><span class="text-surface-400">a</span> attach a folder</span>
				<span><span class="text-surface-400">⌘K</span> search everything</span>
			</p>
		</div>
	</div>
</div>
