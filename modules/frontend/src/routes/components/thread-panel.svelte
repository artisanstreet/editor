<script lang="ts" effect>
	import { Effect } from "effect";
	import type { Project } from "@artisan/protocol";
	import { ReleaseBrowserObjectUrl } from "$lib/browser/object-url";
	import { RouteNavigation } from "$lib/browser/route-navigation";
	import { ComposerDraftStore } from "$lib/composer/draft-store";
	import type { ConversationPlan } from "$lib/conversation/checklist";
	import type { OrchestrationRosterEntry } from "$lib/orchestration/roster";
	import { new_thread_draft_key } from "$lib/root/new-thread-draft";
	import type { RecentProject } from "$lib/root/project-catalog";
	import { WorkspaceRoutePath } from "$lib/root/thread-navigation";
	import HoverPillGroup from "./hover-pill-group.svelte";
	import ProjectFolderPicker from "./project-folder-picker.svelte";
	import ShaderGlassSurface from "./shader-glass-surface.svelte";
	import ThreadAgents from "./thread-agents.svelte";
	import ThreadEnvironmentCard from "./thread-environment-card.svelte";
	import ThreadTerminalsCard from "./thread-terminals-card.svelte";

	let {
		plan,
		agents,
		oninspectagent,
		project,
		project_id,
		project_root_path,
		projects,
		thread_id,
		workspace_id,
	}: {
		readonly plan?: ConversationPlan;
		readonly agents?: ReadonlyArray<OrchestrationRosterEntry>;
		readonly oninspectagent: (entry: OrchestrationRosterEntry) => Effect.Effect<void>;
		readonly project: Project | undefined;
		readonly project_id: string | undefined;
		readonly project_root_path: string | undefined;
		readonly projects: ReadonlyArray<RecentProject>;
		readonly thread_id: string | undefined;
		readonly workspace_id: string | undefined;
	} = $props();

	const navigation = yield* RouteNavigation;
	const composer_drafts = yield* ComposerDraftStore;
	let project_picker_open = $state(false);
	/** One optional read feeds both the branch and the loop; the prop may clear during navigation. */
	const plan_entries = $derived(plan?.entries ?? []);
	const SelectProject = (next: Project) =>
		Effect.gen(function* () {
			const current_draft_key = thread_id ?? new_thread_draft_key(workspace_id);
			const next_draft_key = new_thread_draft_key(next.project_id);
			/**
			 * The picker changes where this in-progress composition belongs; it is
			 * not a New-thread action and therefore must not replace the document.
			 */
			const handoff = yield* composer_drafts.Move(current_draft_key, next_draft_key);
			for (const attachment of handoff.orphaned) {
				yield* ReleaseBrowserObjectUrl(attachment.preview_url).pipe(Effect.ignore);
			}
			yield* navigation.Navigate(WorkspaceRoutePath(next.project_id));
		});
	const OpenProjectPicker = Effect.sync(() => {
		project_picker_open = true;
	});

	const task_tone: Record<ConversationPlan["entries"][number]["state"], string> = {
		active: "font-medium text-foreground marker:text-foreground",
		completed: "text-muted-foreground line-through marker:text-muted-foreground/60",
		pending: "text-muted-foreground marker:text-muted-foreground/60",
		skipped: "text-muted-foreground/70 line-through marker:text-muted-foreground/40",
	};
</script>

<div class="relative flex h-full min-h-0 flex-col p-1">
	<!--
		One hover state spans every card, and each card paints its own copy of the
		pill at the shared geometry, so the highlight slides between the
		environment rows and the agent rows instead of each card fading its own in.
	-->
	<HoverPillGroup class="flex min-h-0 flex-1 flex-col gap-4 [--docs-sidebar-hover-radius:var(--radius-lg)]">
		{#snippet children({ hover })}
		<ThreadEnvironmentCard
			{hover}
			onnewproject={OpenProjectPicker}
			onselectproject={SelectProject}
			{project}
			{project_id}
			{project_root_path}
			{projects}
			{thread_id}
			{workspace_id}
		/>
		{#if agents !== undefined && agents.length > 0}
			<ShaderGlassSurface class="t-resize t-resize-auto min-h-0 max-h-full shrink rounded-xl">
				<div class="flex min-h-0 min-w-0 flex-col p-1">
					<div class="docs-scroll-fade min-h-0 overflow-x-hidden overflow-y-auto">
						<ThreadAgents entries={agents} {hover} oninspect={oninspectagent} {thread_id} />
					</div>
				</div>
			</ShaderGlassSurface>
		{/if}
		<ThreadTerminalsCard {hover} {thread_id} {workspace_id} />
		{#if plan_entries.length > 0}
			<ShaderGlassSurface class="t-resize t-resize-auto min-h-0 max-h-full shrink rounded-xl">
				<div class="flex min-h-0 min-w-0 flex-col p-1">
					<div class="docs-scroll-fade min-h-0 overflow-x-hidden overflow-y-auto">
						<section aria-labelledby="thread-checklist-heading">
							<h2
								id="thread-checklist-heading"
								class="px-2 pt-2 pb-1 text-sm font-medium text-foreground"
							>
								Checklist
							</h2>
							<ul class="min-w-0 list-disc pl-6" aria-labelledby="thread-checklist-heading">
							{#each plan_entries as entry (entry.id)}
									<li
										class={`rounded-lg px-2 py-2 text-sm ${task_tone[entry.state]}`}
										data-state={entry.state}
									>
										<span class="sr-only">{entry.state}: </span>
										<span>{entry.text}</span>
									</li>
								{/each}
							</ul>
						</section>
					</div>
				</div>
			</ShaderGlassSurface>
		{/if}
		{/snippet}
	</HoverPillGroup>
</div>

<ProjectFolderPicker bind:open={project_picker_open} onattached={SelectProject} />
