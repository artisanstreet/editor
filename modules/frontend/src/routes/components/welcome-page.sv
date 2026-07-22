<script lang="ts" effect>
	import { goto } from "$app/navigation";
	import { Effect, Option } from "effect";
	import { IconArrowUpRight as ArrowUpRight, IconMessagePlus as MessagePlus } from "@tabler/icons-svelte";

	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "$lib/components/ui/card";
	import { LiveWorkspaceStore, type LiveWorkspaceSnapshot } from "$lib/live-workspace/store";

	let { live_snapshot }: { live_snapshot: LiveWorkspaceSnapshot } = $props();
	const live_workspace = yield* LiveWorkspaceStore;
	const recent_threads = $derived(
		[...live_snapshot.threads].sort(
			(left, right) => Date.parse(right.last_activity_at) - Date.parse(left.last_activity_at),
		),
	);
	const LastUsed = (date: string) =>
		new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(
			new Date(date),
		);
	const CreateThread = Effect.gen(function* () {
		yield* live_workspace.CreateThread("New chat");
		const latest_snapshot = yield* live_workspace.Snapshot;
		const created_thread_id = Option.getOrUndefined(latest_snapshot.selected_thread_id);
		if (created_thread_id !== undefined)
			yield* Effect.tryPromise(() => goto(`/thread/${created_thread_id}`));
	});
</script>

<div class="welcome-page">
	<section class="welcome-heading">
		<div>
			<p class="eyebrow">Workspace</p>
			<h1>Pick up where you left off.</h1>
			<p class="lede">Recent threads stay here, while every active conversation has its own focused workspace.</p>
		</div>
		<Button variant="outline" class="gap-2" onclick={yield* CreateThread}><MessagePlus size={17} /> New thread</Button>
	</section>

	<div class="recent-card">
	<Card id="recent-threads">
		<CardHeader>
			<div>
				<CardTitle>Recent threads</CardTitle>
				<CardDescription>Your last used conversations, ordered by activity.</CardDescription>
			</div>
			<Badge variant="secondary">{recent_threads.length}</Badge>
		</CardHeader>
		<CardContent>
			{#if recent_threads.length === 0}
				<div class="empty-recent"><MessagePlus size={22} /><p>No threads yet. Create one from the sidebar when you are ready.</p></div>
			{:else}
				<div class="table-scroll">
					<table>
						<thead><tr><th>Thread</th><th>Project</th><th>Status</th><th>Last used</th><th><span class="sr-only">Open</span></th></tr></thead>
						<tbody>
							{#each recent_threads as thread}
								<tr>
									<td><a href={`/thread/${thread.thread_id}`}>{thread.title}</a></td>
									<td>{thread.primary_project?.display_name ?? "No project"}</td>
									<td><Badge variant="outline">{thread.live_status}</Badge></td>
									<td>{LastUsed(thread.last_activity_at)}</td>
									<td><Button href={`/thread/${thread.thread_id}`} variant="ghost" size="icon-sm" aria-label={`Open ${thread.title}`}><ArrowUpRight size={16} /></Button></td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</CardContent>
	</Card>
	</div>
</div>

<style>
	.welcome-page { width: min(100% - 2rem, 70rem); margin: 0 auto; padding: clamp(2rem, 8vh, 6rem) 0 3rem; }
	.welcome-heading { display: flex; align-items: end; justify-content: space-between; gap: 1.5rem; margin-bottom: 2rem; }
	.eyebrow { margin: 0 0 0.5rem; color: var(--text-muted); font-size: 0.6875rem; font-weight: 700; letter-spacing: 0.1em; text-transform: uppercase; }
	h1 { margin: 0; letter-spacing: -0.055em; font-size: clamp(2rem, 5vw, 3.75rem); line-height: 1; }
	.lede { max-width: 37rem; margin: 0.875rem 0 0; color: var(--text-muted); line-height: 1.6; }
	.recent-card :global([data-slot="card"]) { border-radius: 1.5rem; }
	.recent-card :global([data-slot="card-header"]) { display: flex; flex-direction: row; align-items: center; justify-content: space-between; }
	.table-scroll { overflow-x: auto; }
	table { width: 100%; border-collapse: collapse; text-align: left; font-size: 0.8125rem; }
	th { padding: 0.625rem 0.75rem; border-bottom: 1px solid var(--line); color: var(--text-muted); font-size: 0.6875rem; font-weight: 700; letter-spacing: 0.06em; text-transform: uppercase; }
	td { padding: 0.75rem; border-bottom: 1px solid var(--line); color: var(--text-muted); white-space: nowrap; }
	td:first-child { min-width: 16rem; color: var(--text-primary); font-weight: 600; }
	td a { color: inherit; text-decoration: none; }
	td a:hover { text-decoration: underline; text-underline-offset: 0.25rem; }
	tbody tr:last-child td { border-bottom: 0; }
	.empty-recent { display: grid; min-height: 12rem; place-content: center; justify-items: center; gap: 0.75rem; color: var(--text-muted); text-align: center; }
	.empty-recent p { max-width: 20rem; margin: 0; font-size: 0.8125rem; line-height: 1.5; }
	@media (max-width: 640px) { .welcome-page { width: min(100% - 1.5rem, 70rem); padding-top: 2rem; } .welcome-heading { align-items: start; flex-direction: column; } }
</style>
