<script lang="ts" effect>
	import Folder from "@tabler/icons-svelte/icons/folder";
	import HelpCircle from "@tabler/icons-svelte/icons/help-circle";
	import { Effect } from "effect";
	import { RunBrowserDom } from "$lib/browser/dom";
	import type { Project, ProjectRepository, RepositoryDiffSnapshot } from "@artisan/protocol";
	import { MakeFollowHighlight } from "$lib/components/dropdown-highlight";
	import { LipCard } from "$lib/components/ui/lip-card";
	import { Popover, PopoverContent, PopoverTrigger } from "$lib/components/ui/popover";
	import { Select, SelectContent, SelectItem, SelectTrigger } from "$lib/components/ui/select";
	import { ComparisonLabel, DiffCount, DiffFileCount } from "$lib/vcs/diff-presentation";
	import { FormatRecentThreadTime } from "$lib/root/thread-navigation";
	import DiffCountsRow from "../diff-counts-row.svelte";
	import DropdownHoverSurface from "../dropdown-hover-surface.svelte";
	import ShaderGlassSurface from "../shader-glass-surface.svelte";
	import RepositoryLine from "./repository-line.svelte";

	const FollowHighlight = yield* MakeFollowHighlight;
	const BROWSE_VALUE = "artisan:browse-for-folder";
	let {
		active_repository,
		assigning,
		diff_detail_open = $bindable(false),
		disabled,
		existing_projects,
		lip_animate,
		onclose,
		keep_open,
		onopenchange,
		onvaluechange,
		project,
		read_at_ms,
		reportable,
		repositories,
	}: {
		active_repository: ProjectRepository | undefined;
		assigning: boolean;
		diff_detail_open?: boolean;
		disabled: boolean;
		existing_projects: ReadonlyArray<Project>;
		lip_animate: boolean;
		onclose: Effect.Effect<void>;
		keep_open: Effect.Effect<void>;
		onopenchange: (is_open: boolean) => Effect.Effect<void>;
		onvaluechange: (value: string) => Effect.Effect<void>;
		project: Project | undefined;
		read_at_ms: number;
		reportable: RepositoryDiffSnapshot | undefined;
		repositories: ReadonlyMap<string, ProjectRepository>;
	} = $props();

	const PreventFocusRestore = (event: Event) =>
		Effect.gen(function* () {
			yield* RunBrowserDom(() => event.preventDefault());
		});
</script>

<header class="mb-2" aria-label="Thread project">
	<Select type="single" value={project?.project_id ?? ""} {disabled} onOpenChange={yield* onopenchange(event)} onValueChange={yield* onvaluechange(event)}>
		<ShaderGlassSurface use_material={false} class="w-full rounded-2xl bg-(image:--hover-surface-fill) p-2">
			<LipCard variant="glass" open={reportable !== undefined} animate={lip_animate} class="w-full rounded-2xl">
				<SelectTrigger aria-label="Thread project" class="w-full items-center gap-2 rounded-2xl border-transparent bg-transparent p-3 text-left data-[size=default]:h-auto dark:bg-transparent dark:hover:bg-transparent">
					<span class="flex min-w-0 flex-col -space-y-1">
						<span class="truncate text-base font-semibold text-foreground">{project?.display_name ?? "No project"}</span>
						{#if project === undefined}<span class="truncate text-sm text-muted-foreground">Select a folder to pin this thread</span>{:else}<RepositoryLine {project} repository={active_repository} />{/if}
					</span>
				</SelectTrigger>
				{#snippet lip()}
					{#if reportable !== undefined}
						<div class="flex items-center gap-2 px-4 pb-2.5 pt-2 text-xs tabular-nums">
							<span class="text-(--diff-added)">+{DiffCount(reportable.working.lines_added)}</span><span class="text-(--diff-removed)">−{DiffCount(reportable.working.lines_deleted)}</span>{#if reportable.truncated}<span class="text-muted-foreground">+</span>{/if}
							<Popover bind:open={diff_detail_open}>
								<PopoverTrigger aria-label="Diff details" class="rounded-full text-muted-foreground outline-none transition-colors duration-(--duration-fast) ease-in-out hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 motion-reduce:transition-none" onpointerenter={yield* keep_open} onpointerleave={yield* onclose} onfocusin={yield* keep_open} onfocusout={yield* onclose}><HelpCircle class="size-3.5" /></PopoverTrigger>
								<PopoverContent variant="bare" align="start" side="bottom" sideOffset={8} class="w-auto rounded-2xl" onpointerenter={yield* keep_open} onpointerleave={yield* onclose} trapFocus={false} onOpenAutoFocus={yield* PreventFocusRestore(event)} onCloseAutoFocus={yield* PreventFocusRestore(event)}>
									<ShaderGlassSurface strength="strong" class="w-full rounded-2xl"><div class="grid grid-cols-[auto_auto_auto] items-baseline gap-x-5 gap-y-1.5 p-3 text-xs tabular-nums">
										<DiffCountsRow label="Uncommitted" counts={reportable.working} trailing={DiffFileCount(reportable.working.file_count)} />
										<DiffCountsRow label="Staged" counts={reportable.staged} trailing={DiffFileCount(reportable.staged.file_count)} />
										<DiffCountsRow label="Unstaged" counts={reportable.unstaged} trailing={DiffFileCount(reportable.unstaged.file_count)} />
										{#each reportable.comparisons as comparison (comparison.kind)}<DiffCountsRow label={`vs ${comparison.ref}`} note={ComparisonLabel(comparison.kind)} counts={comparison.counts} trailing={`${DiffCount(comparison.ahead)} ahead · ${DiffCount(comparison.behind)} behind`} />{/each}
										{#if reportable.untracked_file_count > 0}<span class="text-muted-foreground">Untracked</span><span class="col-span-2 whitespace-nowrap text-foreground">{DiffFileCount(reportable.untracked_file_count)} — lines not counted</span>{/if}
										{#if reportable.working.binary_file_count > 0}<span class="text-muted-foreground">Binary</span><span class="col-span-2 whitespace-nowrap text-foreground">{DiffFileCount(reportable.working.binary_file_count)} — lines not counted</span>{/if}
										{#if reportable.stash_count > 0}<span class="text-muted-foreground">Stashed</span><span class="col-span-2 whitespace-nowrap text-foreground">{DiffCount(reportable.stash_count)} {reportable.stash_count === 1 ? "entry" : "entries"}</span>{/if}
										{#if reportable.head_committed_at !== undefined}<span class="text-muted-foreground">Last commit</span><span class="col-span-2 whitespace-nowrap text-foreground">{FormatRecentThreadTime(reportable.head_committed_at, read_at_ms)}</span>{/if}
										{#if reportable.truncated}<span class="text-muted-foreground">Note</span><span class="col-span-2 whitespace-nowrap text-foreground">The diff exceeded the read limit; counts are a floor.</span>{/if}
									</div></ShaderGlassSurface>
								</PopoverContent>
							</Popover>
						</div>
					{/if}
				{/snippet}
			</LipCard>
		</ShaderGlassSurface>
		<SelectContent align="start" class="rounded-2xl border-transparent bg-transparent p-0 shadow-none"><ShaderGlassSurface strength="strong" class="rounded-2xl p-1"><DropdownHoverSurface class="[--docs-sidebar-hover-radius:var(--radius-xl)]">{#snippet children({ move_hover })}{#each existing_projects as candidate (candidate.project_id)}<SelectItem value={candidate.project_id} label={candidate.display_name} class="w-full focus:bg-transparent! data-highlighted:bg-transparent! data-highlighted:text-foreground!" {@attach FollowHighlight(move_hover)}><span class="flex min-w-0 flex-col -space-y-1"><span class="truncate text-sm text-foreground">{candidate.display_name}</span><RepositoryLine project={candidate} repository={repositories.get(candidate.project_id)} /></span></SelectItem>{/each}<SelectItem value={BROWSE_VALUE} label="Choose a folder" class="w-full focus:bg-transparent! data-highlighted:bg-transparent! data-highlighted:text-foreground!" {@attach FollowHighlight(move_hover)}><Folder class="size-4 shrink-0 text-muted-foreground" /><span class="text-sm">Choose a folder…</span></SelectItem>{/snippet}</DropdownHoverSurface></ShaderGlassSurface></SelectContent>
	</Select>
</header>
