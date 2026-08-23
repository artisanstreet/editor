<script lang="ts" effect>
	import { navigating, page } from "$app/state";
	import { untrack } from "svelte";
	import { Effect, Option, Stream } from "effect";
	import type { ThreadListItem } from "@artisan/protocol";
	import { RouteNavigation } from "$lib/browser/route-navigation";
	import { EditorRouteTargetForThread } from "$lib/editor/workspace-identity";
	import {
		ResolveThreadRoute,
		ThreadRouteHasWorkspace,
		ThreadRouteOwnsTarget,
	} from "$lib/root/thread-navigation";
	import {
		WorkspaceCatalogController,
		type WorkspaceCatalogState,
	} from "$lib/root/workspace-catalog-controller";
	import EditorRoute from "../../../components/editor-route.svelte";

	let {
		thread_id: route_thread_id,
		workspace_id: route_workspace_id,
	}: {
		readonly thread_id: string;
		readonly workspace_id: string;
	} = $props();
	/** Two-step sources prevent the compiler from folding these snapshots back into prop-deriveds. */
	let route_id = $state.raw("");
	let workspace_id = $state.raw("");
	route_id = untrack(() => route_thread_id);
	workspace_id = untrack(() => route_workspace_id);

	const navigation = yield* RouteNavigation;
	const workspace_catalog = yield* WorkspaceCatalogController;
	let catalog_state = $state.raw<WorkspaceCatalogState>(yield* workspace_catalog.Current);
	let active_thread = $state.raw<ThreadListItem | undefined>();

	/**
	 * Thread-list updates arrive for every thread, and this scope can outlive
	 * its route while a replacement renders. Reconciling while another thread —
	 * or the conversation surface of this one — owns the URL (or a navigation
	 * is heading there) would steal focus back and cancel that navigation.
	 */
	let owning_route = $state.raw<string | null>(null);
	owning_route = untrack(() => page.route.id);
	const route_owns_thread = () =>
		navigating.to === null
			? ThreadRouteOwnsTarget(
					{ route_id: owning_route, thread_route_id: route_id },
					{ route_id: page.route.id, thread_param: page.params.thread },
				)
			: ThreadRouteOwnsTarget(
					{ route_id: owning_route, thread_route_id: route_id },
					{
						route_id: navigating.to.route.id,
						thread_param: navigating.to.params?.thread,
					},
				);

	/**
	 * Workspace authority can change while the editor is mounted. Hide the
	 * editor before navigating so no stale component can issue another file read
	 * against a workspace the thread no longer owns.
	 */
	const ReconcileRoute = Effect.gen(function* () {
		if (!route_owns_thread()) return;
		if (!catalog_state.threads_loaded) return;
		const thread = Option.getOrUndefined(ResolveThreadRoute(catalog_state.threads, route_id));
		if (thread === undefined) {
			active_thread = undefined;
			yield* navigation.Navigate("/", {
					keepFocus: true,
					noScroll: true,
					replaceState: true,
				});
			return;
		}

		const target = EditorRouteTargetForThread(
			thread,
			page.url.searchParams.get("file") ?? undefined,
		);
		const route_matches =
			target.type === "editor" &&
			ThreadRouteHasWorkspace(thread, workspace_id) &&
			`${page.url.pathname}${page.url.search}` === target.path;
		if (!route_matches) {
			active_thread = undefined;
			yield* navigation.Navigate(target.path, {
					keepFocus: true,
					noScroll: true,
					replaceState: true,
				});
			return;
		}

		active_thread = thread;
	});

	const ApplyCatalogState = (next: WorkspaceCatalogState) =>
		Effect.gen(function* () {
			catalog_state = next;
			yield* ReconcileRoute;
		});

	yield* ReconcileRoute;
	yield* workspace_catalog.Changes.pipe(
		Stream.runForEach(ApplyCatalogState),
		Effect.forkScoped,
	);
</script>

{#if active_thread?.primary_project !== undefined}
	<EditorRoute
		thread_id={active_thread?.thread_id}
		workspace_id={active_thread?.primary_project?.project_id}
	/>
{:else if !catalog_state.threads_loaded}
	<div class="flex h-full min-h-0 items-center justify-center" role="status">
		<p class="text-sm text-muted-foreground">Loading thread…</p>
	</div>
{/if}
