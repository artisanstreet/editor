<script lang="ts" effect>
	import { page } from "$app/state";
	import FileOff from "@tabler/icons-svelte/icons/file-off";
	import { Clock, Effect, Schedule } from "effect";
	import type { WorkspaceChange } from "@artisan/protocol";
	import { ArtisanClient } from "@artisan/transport/client";
	import ArtisanLogo from "$lib/components/artisan-logo.svelte";
	import { Skeleton } from "$lib/components/ui/skeleton";
	import { resolve_file_icon } from "$lib/conversation/file-icon";
	import {
		EditorFileKeyForFile,
		EditorService,
		type EditorWorkspaceFile,
	} from "$lib/editor/service";
	import { EditorRoutePath } from "$lib/editor/workspace-identity";
	import { EditorFileFromRead } from "$lib/editor/workspace-session";
	import { MakeLatestRequestGate } from "$lib/lifecycle/latest-request-gate";
	import { FormatRecentThreadTime } from "$lib/root/thread-navigation";
	import EditorSurface from "$lib/components/editor/surface.svelte";

	let {
		thread_id,
		workspace_id,
	}: {
		readonly thread_id: string | undefined;
		readonly workspace_id: string | undefined;
	} = $props();

	/**
	 * The editor route.
	 *
	 * The URL carries the whole session: `?file=` names the open path and
	 * `?workspace=` pins the workspace, so a refresh, a deep link, and a click in
	 * the sidebar tree all take the same path through this component. Tabs are
	 * derived from what the service has open rather than tracked separately.
	 */

	const client = yield* ArtisanClient;
	const editor = yield* EditorService;
	const open_requests = yield* MakeLatestRequestGate;

	/**
	 * Mirrors the editor service's bounded resident set by path. The service owns
	 * eviction; this route only keeps enough identity to re-read a clean document
	 * after it falls out of the LRU.
	 */
	let retained_files = $state.raw<ReadonlyMap<string, EditorWorkspaceFile>>(new Map());
	let opening_path = $state<string | undefined>(undefined);
	/**
	 * Why each path could not be opened, kept per path rather than as one
	 * message. A file that cannot be read is a property of that file, and the
	 * editor must not keep showing the previously attached document underneath a
	 * corner error — that reads as though the unreadable file opened fine.
	 */
	let open_failures = $state.raw<ReadonlyMap<string, string>>(new Map());

	const active_path = $derived(page.url.searchParams.get("file") ?? undefined);
	const active_failure = $derived(
		active_path === undefined ? undefined : open_failures.get(active_path),
	);
	const retained_file = $derived(
		active_path === undefined ? undefined : retained_files.get(active_path),
	);

	const ActivateFile = (path: string, file: EditorWorkspaceFile, generation: number) =>
		Effect.gen(function* () {
			if (!(yield* open_requests.IsCurrent(generation))) return;
			yield* editor.Activate(file);
			if (!(yield* open_requests.IsCurrent(generation))) return;
			const session = yield* editor.Current;
			const next_retained_files = new Map(
				[...retained_files].filter(([, retained]) =>
					session.open_file_keys.has(EditorFileKeyForFile(retained)),
				),
			);
			next_retained_files.set(path, file);
			retained_files = next_retained_files;
			open_failures = new Map(
				[...open_failures].filter(([failed_path]) => failed_path !== path),
			);
		});

	const OpenPath = (path: string, target_workspace_id: string, generation: number) =>
		client
			.ReadWorkspaceFile({ path, workspace_id: target_workspace_id })
			.pipe(
				Effect.map(EditorFileFromRead),
				Effect.flatMap((file) => ActivateFile(path, file, generation)),
			Effect.catch((error) =>
				Effect.gen(function* () {
					if (!(yield* open_requests.IsCurrent(generation))) return;
					open_failures = new Map(open_failures).set(path, error.message);
				}),
			),
			Effect.andThen(
				Effect.gen(function* () {
					if (yield* open_requests.IsCurrent(generation)) opening_path = undefined;
				}),
			),
		);

	/**
	 * URL changes invalidate older reads immediately. Opening is forked so the
	 * route paints its skeleton before filesystem I/O answers; a resident file
	 * reactivates locally without another Forge request.
	 */
	const ReconcileOpenPath = (
		path: string | undefined,
		target_workspace_id: string | undefined,
		retained: EditorWorkspaceFile | undefined,
	) =>
		Effect.gen(function* () {
			const generation = yield* open_requests.Begin;
			if (path === undefined || target_workspace_id === undefined) {
				opening_path = undefined;
				return;
			}
			opening_path = path;
			open_failures = new Map(
				[...open_failures].filter(([failed_path]) => failed_path !== path),
			);
			yield* (retained === undefined
				? OpenPath(path, target_workspace_id, generation)
				: ActivateFile(path, retained, generation).pipe(
						Effect.andThen(
							Effect.gen(function* () {
								if (yield* open_requests.IsCurrent(generation)) {
									opening_path = undefined;
								}
							}),
						),
					))
				.pipe(Effect.forkScoped);
		});
	yield* ReconcileOpenPath(active_path, workspace_id, retained_file);

	/**
	 * The empty state shows the files this thread's runs actually touched,
	 * newest first — the applied-change records already carry path and time.
	 * Loaded like the workspace header: retried through the transport cold
	 * start, then given up quietly; a missing table is not worth a banner.
	 */
	let recent_changes = $state.raw<ReadonlyArray<WorkspaceChange>>([]);
	const now_ms = yield* Clock.currentTimeMillis;
	const ColdStartRetrySchedule = Schedule.exponential("100 millis").pipe(
		Schedule.upTo({ duration: "5 seconds" }),
	);
	const most_recent_per_path = (
		changes: ReadonlyArray<WorkspaceChange>,
	): ReadonlyArray<WorkspaceChange> => {
		const seen = new Set<string>();
		const unique: Array<WorkspaceChange> = [];

		for (const change of changes) {
			if (seen.has(change.path)) continue;
			seen.add(change.path);
			unique.push(change);
			if (unique.length === 8) break;
		}

		return unique;
	};
	const LoadRecentChanges = (
		path: string | undefined,
		target_thread_id: string | undefined,
		target_workspace_id: string | undefined,
	) =>
		Effect.gen(function* () {
			if (
				path !== undefined ||
				target_thread_id === undefined ||
				target_workspace_id === undefined
			)
				return;
			const result = yield* client
				.ListWorkspaceChanges({
					thread_id: target_thread_id,
					workspace_id: target_workspace_id,
				})
				.pipe(Effect.retry({ schedule: ColdStartRetrySchedule }));
			recent_changes = most_recent_per_path(result.changes);
		}).pipe(Effect.ignore);
	yield* LoadRecentChanges(active_path, thread_id, workspace_id).pipe(Effect.forkScoped);

	const route_workspace = $derived(page.params.workspace);
	const route_thread = $derived(page.params.thread);
	const file_name = (path: string) => path.split(/[\\/]/u).at(-1) ?? path;
	const file_parent = (path: string) => path.split(/[\\/]/u).slice(0, -1).join("/");
</script>

<svelte:head>
	<title>{active_path ?? "Editor"} · Artisan Editor</title>
</svelte:head>

<div class="flex h-full min-h-0 min-w-0 flex-col">
	{#if workspace_id === undefined}
		<!-- The editor never guesses a workspace; without one in the URL there is nothing to edit. -->
		<div class="flex min-h-0 flex-1 items-center justify-center">
			<p class="text-sm text-muted-foreground">Open a workspace to start editing.</p>
		</div>
	{:else if active_path === undefined}
		<div class="flex min-h-0 flex-1 items-center justify-center p-6">
			{#if recent_changes.length === 0 || route_workspace === undefined || route_thread === undefined}
				<!-- Nothing changed in this thread yet: the mark keeps the room, quietly. -->
				<div class="text-muted-foreground/60 select-none">
					<ArtisanLogo size={56} />
				</div>
				<span class="sr-only">Select a file to start editing.</span>
			{:else}
				<div class="w-full max-w-md min-w-0">
					<p class="mb-2 text-xs font-medium text-muted-foreground">Recently changed files</p>
					<div class="docs-scroll-fade max-h-[calc(12rem+3px)] min-w-0 overflow-y-auto">
						<table
							class="w-full table-fixed border-collapse text-left"
							aria-label="Recently changed files"
						>
							<thead class="sr-only">
								<tr><th>File</th><th>Changed</th></tr>
							</thead>
							<tbody>
								{#each recent_changes as change (change.change_id)}
									<tr class="group border-b border-border last:border-b-0">
										<td class="min-w-0 p-0">
											<a
												href={EditorRoutePath(route_workspace, route_thread, change.path)}
												class="flex min-w-0 items-center gap-2 py-3 font-medium text-foreground outline-none transition-colors duration-(--duration-fast) ease-in-out group-hover:text-foreground-extra group-focus-within:text-foreground-extra motion-reduce:transition-none"
											>
												<img
													src={resolve_file_icon(change.path)}
													alt=""
													aria-hidden="true"
													class="size-4 shrink-0"
												/>
												<span class="shrink-0">{file_name(change.path)}</span>
												{#if file_parent(change.path) !== ""}
													<span class="min-w-0 truncate text-xs font-normal text-muted-foreground">
														{file_parent(change.path)}
													</span>
												{/if}
											</a>
										</td>
										<td
											class="w-28 p-0 pl-4 text-right text-xs text-muted-foreground transition-colors duration-(--duration-fast) ease-in-out group-hover:text-foreground-extra group-focus-within:text-foreground-extra motion-reduce:transition-none"
										>
											<span class="whitespace-nowrap">
												{FormatRecentThreadTime(change.updated_at, now_ms)}
											</span>
										</td>
									</tr>
								{/each}
							</tbody>
						</table>
					</div>
				</div>
			{/if}
		</div>
	{:else if opening_path === active_path}
		<div
			class="flex min-h-0 flex-1 flex-col gap-3 p-6"
			aria-label={`Loading ${active_path}`}
			role="status"
		>
			<Skeleton class="h-4 w-2/5" />
			<Skeleton class="h-4 w-4/5" />
			<Skeleton class="h-4 w-3/4" />
			<Skeleton class="h-4 w-5/6" />
			<Skeleton class="h-4 w-1/2" />
		</div>
	{:else if active_failure !== undefined}
		<!--
			The surface is unmounted rather than left showing the previously
			attached document. The service keeps every open document and its view
			state across detach, so re-opening a readable file restores it exactly.
		-->
		<div class="flex min-h-0 flex-1 items-center justify-center p-6" role="alert">
			<div class="flex max-w-md min-w-0 flex-col items-center gap-2 text-center">
				<FileOff class="size-6 shrink-0 text-muted-foreground" aria-hidden="true" />
				<p class="text-sm font-medium text-foreground">This file can&rsquo;t be displayed</p>
				<p class="text-pretty text-xs text-muted-foreground">{active_failure}</p>
				<p class="truncate text-xs text-muted-foreground/70" title={active_path}>
					{active_path}
				</p>
			</div>
		</div>
	{:else}
		<EditorSurface label={active_path} />
	{/if}
</div>
