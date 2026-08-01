<script lang="ts" effect>
	import { page } from "$app/state";
	import { Effect } from "effect";
	import { ArtisanClient } from "@artisan/transport/client";
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
	const LoadDirectory = (parent: string) =>
		Effect.gen(function* () {
			if (workspace_id === undefined) {
				failure = "Open a workspace to browse its files.";
				return;
			}
			const discovered = yield* client.ListWorkspaceFiles({
				depth: 1,
				limit: 1_000,
				workspace_id,
				...(parent === workspace_tree_root ? {} : { prefix: parent }),
			});
			tree = MergeWorkspaceEntries(tree, WorkspaceEntriesByParent(discovered.entries), parent);
			failure = undefined;
		}).pipe(
			Effect.catch((error) =>
				Effect.gen(function* () {
					failure = error.message;
				}),
			),
		);

	if (directory_requests.length > 0) {
		const [parent, ...remaining] = directory_requests;
		directory_requests = remaining;
		yield* LoadDirectory(parent);
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
			<p class="px-1 text-xs text-muted-foreground">Loading files…</p>
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
