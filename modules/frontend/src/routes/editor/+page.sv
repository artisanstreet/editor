<script lang="ts" effect>
	import { page } from "$app/state";
	import FileOff from "@tabler/icons-svelte/icons/file-off";
	import { Effect, Queue } from "effect";
	import { ArtisanClient } from "@artisan/transport/client";
	import { MakeEditorSurfaceMount } from "$lib/editor/mount";
	import { EditorFileKeyForFile, EditorService } from "$lib/editor/service";
	import { EditorFileFromRead } from "$lib/editor/workspace-session";
	import EditorSurface from "$lib/components/editor/surface.sv";
	import { EditorWorkspaceId } from "$lib/editor/workspace-identity";

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

	let open_files = $state.raw<ReadonlyArray<{ path: string; revision: string }>>([]);
	/**
	 * Why each path could not be opened, kept per path rather than as one
	 * message. A file that cannot be read is a property of that file, and the
	 * editor must not keep showing the previously attached document underneath a
	 * corner error — that reads as though the unreadable file opened fine.
	 */
	let open_failures = $state.raw<ReadonlyMap<string, string>>(new Map());

	const workspace_id = $derived(EditorWorkspaceId(page.url));
	const active_path = $derived(page.url.searchParams.get("file") ?? undefined);
	const active_file = $derived(open_files.find((file) => file.path === active_path));
	const active_failure = $derived(
		active_path === undefined ? undefined : open_failures.get(active_path),
	);

	const mount = MakeEditorSurfaceMount(editor);

	const OpenPath = (path: string) =>
		Effect.gen(function* () {
			if (workspace_id === undefined) return;
			const read = yield* client.ReadWorkspaceFile({ path, workspace_id });
			const file = EditorFileFromRead(read);
			yield* editor.Activate(file);
			open_files = [
				...open_files.filter((candidate) => candidate.path !== path),
				{ path, revision: file.revision },
			];
			open_failures = new Map(
				[...open_failures].filter(([failed_path]) => failed_path !== path),
			);
		}).pipe(
			Effect.catch((error) =>
				Effect.sync(() => {
					open_failures = new Map(open_failures).set(path, error.message);
				}),
			),
		);

	const open_requests = yield* Queue.dropping<string>(1);
	yield* Queue.take(open_requests).pipe(
		Effect.flatMap(OpenPath),
		Effect.forever,
		Effect.forkScoped,
	);

	/** The URL is the source of truth, so opening happens as a reaction to it. */
	$effect(() => {
		const path = active_path;
		if (path === undefined || workspace_id === undefined) return;
		if (open_files.some((file) => file.path === path)) return;
		Queue.offerUnsafe(open_requests, path);
	});
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
		<div class="flex min-h-0 flex-1 items-center justify-center">
			<p class="text-sm text-muted-foreground">Select a file to start editing.</p>
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
		<EditorSurface {mount} label={active_path} />
	{/if}
</div>
