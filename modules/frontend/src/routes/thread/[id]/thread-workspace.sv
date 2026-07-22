<script lang="ts" effect>
	import { page } from "$app/state";
	import { Effect, Option, Stream } from "effect";
	import ArrowLeft from "@tabler/icons-svelte/icons/arrow-left";
	import Inspector from "@tabler/icons-svelte/icons/layout-sidebar-right";

	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Sheet, SheetContent, SheetTrigger } from "$lib/components/ui/sheet";
	import { LiveWorkspaceStore, type LiveWorkspaceSnapshot } from "$lib/live-workspace/store";

	import MainPane from "../../components/main-pane.sv";
	import RightPane from "../../components/right-pane.sv";

	const live_workspace = yield* LiveWorkspaceStore;
	const thread_id = page.params.id;
	let live_snapshot = $state.raw<LiveWorkspaceSnapshot>(yield* live_workspace.Snapshot);
	let inspector_open = $state(false);

	/** Route entry is the authority for the active thread and starts its scoped projections. */
	yield* live_workspace.SelectThread(thread_id);
	live_snapshot = yield* live_workspace.Snapshot;
	yield* Stream.runForEach(live_workspace.Changes, (next_snapshot) =>
		Effect.sync(() => {
			live_snapshot = next_snapshot;
		}),
	).pipe(Effect.forkScoped);

	const selected_thread = $derived(
		live_snapshot.threads.find((thread) => thread.thread_id === thread_id),
	);
	const selected_project = $derived(selected_thread?.primary_project);
	const selected_thread_id = $derived(Option.getOrUndefined(live_snapshot.selected_thread_id));
</script>

<section class="flex h-full min-h-0 flex-col gap-2" aria-label="Thread workspace">
	<header class="flex min-h-12 items-center gap-3 rounded-xl border bg-card px-3 shadow-sm">
		<Button href="/" variant="ghost" size="icon-sm" aria-label="Back to recent threads" title="Back to recent threads">
			<ArrowLeft size={17} aria-hidden="true" />
		</Button>
		<div class="min-w-0 flex-1">
			<h1 class="truncate text-sm font-semibold">{selected_thread?.title ?? "Thread"}</h1>
			<p class="truncate text-xs text-muted-foreground">{selected_project?.display_name ?? "No project assigned"}</p>
		</div>
		{#if selected_thread !== undefined}
			<Badge variant="secondary">{selected_thread.live_status}</Badge>
		{/if}
		<Sheet bind:open={inspector_open}>
			<SheetTrigger>
				{#snippet child({ props })}
					<Button {...props} variant="outline" size="icon-sm" aria-label="Open thread inspector" title="Thread inspector">
						<Inspector size={17} aria-hidden="true" />
					</Button>
				{/snippet}
			</SheetTrigger>
			<SheetContent side="right" class="w-[min(25rem,calc(100vw-1.5rem))] p-0" aria-label="Thread inspector">
				<RightPane instance_id="thread-inspector" {live_snapshot} controller={live_workspace} />
			</SheetContent>
		</Sheet>
	</header>

	{#if selected_thread_id === thread_id || selected_thread !== undefined}
		<div class="min-h-0 flex-1">
			<MainPane
				{live_snapshot}
				actions={live_workspace.Actions}
				on_send_live_message={live_workspace.SendMessage}
				on_refresh_workspace_files={live_workspace.RefreshWorkspaceFiles}
				on_read_workspace_file={live_workspace.ReadWorkspaceFile}
				on_replace_workspace_file={live_workspace.ReplaceWorkspaceFile}
				on_select_orchestration_group={live_workspace.SelectOrchestrationGroup}
			/>
		</div>
	{:else}
		<div class="grid min-h-0 flex-1 place-content-center rounded-xl border bg-card p-8 text-center">
			<p class="text-sm font-medium">Thread unavailable</p>
			<p class="mt-1 text-xs text-muted-foreground">This thread may have been archived or removed in another session.</p>
		</div>
	{/if}
</section>
