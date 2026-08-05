<script lang="ts" effect>
	import { page } from "$app/state";
	import { Effect, Option, Schedule } from "effect";
	import { ArtisanClient } from "@artisan/transport/client";
	import { Skeleton } from "$lib/components/ui/skeleton";
	import { RouteNavigation } from "$lib/browser/route-navigation";
	import { EditorRoutePath } from "$lib/editor/workspace-identity";
	import {
		MergeWorkspaceEntries,
		WorkspaceEntriesByParent,
		workspace_tree_root,
		type WorkspaceTreeEntry,
	} from "$lib/editor/workspace-session";
	import WorkspaceFileTree from "./workspace-file-tree.sv";

	/**
	 * The editor's file tree, in the inspector column beside the document.
	 *
	 * It lives here rather than in the sidebar because the sidebar is navigation —
	 * where you go — while the tree is part of the workspace you are already in.
	 * That also lets the sidebar collapse to its rail without taking the tree
	 * with it, which is how the editor is actually used.
	 */

	const client = yield* ArtisanClient;
	const navigation = yield* RouteNavigation;
	let tree = $state.raw<ReadonlyMap<string, ReadonlyArray<WorkspaceTreeEntry>>>(new Map());
	let expanded = $state.raw<ReadonlySet<string>>(new Set());
	let failure = $state<string | undefined>(undefined);
	let directory_requests = $state.raw<ReadonlyArray<string>>([workspace_tree_root]);

	const active_file = $derived(page.url.searchParams.get("file") ?? undefined);
	const workspace_id = $derived(page.params.workspace);
	const thread_id = $derived(page.params.thread);

	/**
	 * One directory at a time: `depth: 1` under the opened prefix. A repository of
	 * any size therefore costs one small listing to mount, and each folder costs
	 * one more the first time it is opened.
	 */
	/** The panel mounts with the shell, so the first listing rides out the transport cold start. */
	const ColdStartRetrySchedule = Schedule.exponential("100 millis").pipe(
		Schedule.upTo({ duration: "5 seconds" }),
	);

	const LoadDirectory = (parent: string) =>
		Effect.gen(function* () {
			if (workspace_id === undefined) {
				failure = "Open a workspace to browse its files.";
				return;
			}
			/**
			 * Bounded, because this effect runs at the component's top level: a
			 * Forge that never answers would otherwise leave a fiber pending for
			 * the life of the route, and the async renderer refuses to navigate
			 * away from work it still considers unfinished — one dropped reply
			 * froze every later route change until the page was reloaded.
			 */
			const discovered = yield* client
				.ListWorkspaceFiles({
					depth: 1,
					limit: 1_000,
					workspace_id,
					...(parent === workspace_tree_root ? {} : { prefix: parent }),
				})
				.pipe(
					Effect.retry({ schedule: ColdStartRetrySchedule }),
					Effect.timeoutOption("10 seconds"),
				);
			if (Option.isNone(discovered)) {
				failure = "The file listing did not answer. Retry once Forge is reachable.";
				return;
			}
			tree = MergeWorkspaceEntries(
				tree,
				WorkspaceEntriesByParent(discovered.value.entries),
				parent,
			);
			failure = undefined;
		}).pipe(
			Effect.catch((error) =>
				Effect.gen(function* () {
					failure = error.message;
				}),
			),
		);

	/**
	 * The queue is a reactive input of this yield site, so it must only be
	 * written after the load completes: popping it first invalidates the site
	 * mid-flight, SER interrupts the request, and the rerun sees an empty queue
	 * — the tree then never loads.
	 */
	if (directory_requests.length > 0) {
		const [parent, ...remaining] = directory_requests;
		yield* LoadDirectory(parent);
		directory_requests = remaining;
	}

	const ToggleDirectory = (path: string) =>
		Effect.gen(function* () {
			const next = new Set(expanded);
			if (next.has(path)) next.delete(path);
			else {
				next.add(path);
				if (!tree.has(path)) directory_requests = [...directory_requests, path];
			}
			expanded = next;
		});

	const OpenFile = (path: string) =>
		Effect.gen(function* () {
			if (workspace_id === undefined || thread_id === undefined) return;
			yield* navigation.Navigate(EditorRoutePath(workspace_id, thread_id, path));
		});

</script>

<div class="flex h-full min-h-0 flex-col gap-2 p-3">
	<p class="shrink-0 px-1 text-xs font-medium text-muted-foreground">Files</p>

	<!-- Same fade the transcript and the sidebar use, so every scrolling column reads alike. -->
	<div class="docs-scroll-fade min-h-0 flex-1 overflow-y-auto">
		{#if failure !== undefined}
			<p class="px-1 text-xs text-muted-foreground">{failure}</p>
		{:else if !tree.has(workspace_tree_root)}
			<div class="flex flex-col gap-2.5 px-1 pt-1" aria-label="Loading files" role="status">
				<Skeleton class="h-4 w-3/4" />
				<Skeleton class="h-4 w-1/2" />
				<Skeleton class="h-4 w-2/3" />
				<Skeleton class="h-4 w-3/5" />
				<Skeleton class="h-4 w-2/5" />
				<Skeleton class="h-4 w-1/2" />
			</div>
		{:else if (tree.get(workspace_tree_root) ?? []).length === 0}
			<p class="px-1 text-xs text-muted-foreground">This project has no files.</p>
		{:else}
			<div class="flex min-w-0 flex-col">
				<WorkspaceFileTree
					active_path={active_file}
					children_by_path={tree}
					{expanded}
					onopen={OpenFile}
					ontoggle={ToggleDirectory}
				/>
			</div>
		{/if}
	</div>
</div>
