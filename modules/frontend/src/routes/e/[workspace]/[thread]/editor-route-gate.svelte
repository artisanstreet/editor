<script lang="ts" effect>
	import { navigating, page } from "$app/state";
	import { untrack } from "svelte";
	import { Effect, Option } from "effect";
	import type { ThreadListItem } from "@artisan/protocol";
	import { ArtisanClient, type ThreadListUpdate } from "@artisan/transport/client";
	import { RunAuthoritativeSubscription } from "$lib/conversation/subscription";
	import { RouteNavigation } from "$lib/browser/route-navigation";
	import { EditorRouteTargetForThread } from "$lib/editor/workspace-identity";
	import {
		ApplyRootThreadListUpdate,
		ResolveThreadRoute,
		ThreadRouteHasWorkspace,
		ThreadRouteOwnsTarget,
	} from "$lib/root/thread-navigation";
	import EditorRoute from "../../../components/editor-route.svelte";

	let {
		thread_id: route_thread_id,
		workspace_id: route_workspace_id,
	}: {
		readonly thread_id: string;
		readonly workspace_id: string;
	} = $props();
	const route_id = untrack(() => route_thread_id);
	const workspace_id = untrack(() => route_workspace_id);

	const client = yield* ArtisanClient;
	const navigation = yield* RouteNavigation;
	let threads = $state.raw<ReadonlyArray<ThreadListItem>>(yield* client.ListThreads);
	let active_thread = $state.raw<ThreadListItem | undefined>();

	/**
	 * Thread-list updates arrive for every thread, and this scope can outlive
	 * its route while a replacement renders. Reconciling while another thread —
	 * or the conversation surface of this one — owns the URL (or a navigation
	 * is heading there) would steal focus back and cancel that navigation.
	 */
	const owning_route = untrack(() => page.route.id);
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
		const thread = Option.getOrUndefined(ResolveThreadRoute(threads, route_id));
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

	const ApplyThreadListUpdate = (update: ThreadListUpdate) =>
		Effect.gen(function* () {
			threads = ApplyRootThreadListUpdate(threads, update);
			yield* ReconcileRoute;
		});

	const RefreshThreads = Effect.gen(function* () {
		const next_threads = yield* client.ListThreads;
		yield* ApplyThreadListUpdate({
			journal_sequence: 0,
			threads: next_threads,
			type: "snapshot" as const,
		});
	});

	yield* ReconcileRoute;
	yield* RunAuthoritativeSubscription(
		client.SubscribeThreadList,
		ApplyThreadListUpdate,
		RefreshThreads,
	).pipe(Effect.forkScoped);
</script>

{#if active_thread?.primary_project !== undefined}
	<EditorRoute
		thread_id={active_thread.thread_id}
		workspace_id={active_thread.primary_project.project_id}
	/>
{/if}
