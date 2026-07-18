<script lang="ts" effect>
	import { Effect, Option, Stream } from "effect";
	import {
		IconArrowUp as ArrowUp,
		IconRefresh as Refresh,
		IconWifiOff as WifiOff,
	} from "@tabler/icons-svelte";

	import { LiveWorkspaceStore } from "./store";
	import { Button } from "$lib/components/ui/button";
	import { Card, CardContent, CardHeader, CardTitle } from "$lib/components/ui/card";
	import { Textarea } from "$lib/components/ui/textarea";

	const store = yield* LiveWorkspaceStore;
	let snapshot = $state.raw(yield* store.Snapshot);
	let draft = $state("");
	yield* Stream.runForEach(store.Changes, (next_snapshot) =>
		Effect.sync(() => {
			snapshot = next_snapshot;
		}),
	).pipe(Effect.forkScoped);

	const RefreshWorkspace = Effect.gen(function* () {
		yield* store.Refresh;
		snapshot = yield* store.Snapshot;
	});

	const SelectThread = (thread_id: string) =>
		Effect.gen(function* () {
			yield* store.SelectThread(thread_id);
			snapshot = yield* store.Snapshot;
		});

	const SendMessage = Effect.gen(function* () {
		yield* store.SendMessage(draft);
		snapshot = yield* store.Snapshot;
		if (Option.isNone(snapshot.error)) draft = "";
	});

	const HandleComposerKey = (key: string, meta_key: boolean, ctrl_key: boolean) =>
		Effect.gen(function* () {
			if (key === "Enter" && (meta_key || ctrl_key)) {
				yield* SendMessage;
			}
		});

	const selected_thread_id = $derived(Option.getOrUndefined(snapshot.selected_thread_id));
</script>

<main class="grid min-h-dvh grid-cols-1 gap-2 bg-background p-2 text-foreground lg:grid-cols-[17rem_minmax(0,1fr)_21rem]" aria-busy={snapshot.phase === "connecting" || snapshot.phase === "reconnecting"}>
	<Card class="min-h-0">
		<CardHeader class="flex-row items-center justify-between space-y-0 p-3">
			<CardTitle class="text-sm">Artisan</CardTitle>
			<Button variant="ghost" size="icon-sm" aria-label="Refresh live workspace" title="Refresh live workspace" onclick={yield* RefreshWorkspace}><Refresh size={16} /></Button>
		</CardHeader>
		<CardContent class="grid gap-2 p-3 pt-0">
			<p class="text-xs text-muted-foreground">{snapshot.phase}</p>
			{#if snapshot.threads.length === 0}
				<p class="text-sm text-muted-foreground">{snapshot.phase === "error" ? "The desktop session is unavailable." : "No threads yet."}</p>
			{:else}
				<nav aria-label="Threads" class="grid gap-1">
					{#each snapshot.threads as thread}
						<Button variant={thread.thread_id === selected_thread_id ? "secondary" : "ghost"} class="h-auto justify-start whitespace-normal text-left" aria-current={thread.thread_id === selected_thread_id ? "page" : undefined} onclick={yield* SelectThread(thread.thread_id)}>
							<span class="grid gap-1"><span>{thread.title}</span><small class="text-muted-foreground">{thread.live_status}</small></span>
						</Button>
					{/each}
				</nav>
			{/if}
		</CardContent>
	</Card>

	<section class="min-h-0" aria-label="Current workspace">
		<Card class="h-full">
			<CardHeader class="p-4">
				<CardTitle>Live session</CardTitle>
				<p class="text-sm text-muted-foreground">Backend-owned projections only. The fixture view remains at <code>/visual-fixtures</code>.</p>
			</CardHeader>
			<CardContent class="grid gap-4 p-4 pt-0">
				{#if Option.isSome(snapshot.thread_work)}
					<div class="grid gap-1"><strong>{snapshot.thread_work.value.display_name}</strong><span class="text-sm text-muted-foreground">{snapshot.thread_work.value.role} · {snapshot.thread_work.value.status}</span><code class="text-xs text-muted-foreground">{snapshot.thread_work.value.engine_id}</code></div>
				{:else if selected_thread_id !== undefined}
					<p class="text-sm text-muted-foreground">No active work is projected for this thread.</p>
				{:else}
					<p class="text-sm text-muted-foreground">Select or create a thread when the backend exposes one.</p>
				{/if}
				<label class="grid gap-2"><span class="text-sm font-medium">Message</span><Textarea bind:value={draft} aria-label="Message active thread" placeholder="Message the active Codex run" onkeydown={yield* HandleComposerKey(event.key, event.metaKey, event.ctrlKey)} /><Button class="justify-self-end" disabled={draft.trim().length === 0 || !Option.isSome(snapshot.thread_work)} onclick={yield* SendMessage}>Send <ArrowUp size={16} /></Button></label>
				<div class="rounded-md border p-3 text-sm text-muted-foreground">Transcript, file discovery, graph discovery, previews, and usage are intentionally unavailable until their authoritative backend projections are connected.</div>
			</CardContent>
		</Card>
	</section>

	<Card class="min-h-0">
		<CardHeader class="p-3"><CardTitle class="text-sm">Session</CardTitle></CardHeader>
		<CardContent class="grid gap-4 p-3 pt-0">
			<div class="grid gap-1"><strong class="text-xs">Guidance</strong><span class="text-sm text-muted-foreground">{Option.isSome(snapshot.global_guidance) ? "Backend guidance is loaded." : "Unavailable"}</span></div>
			<div class="grid gap-1"><strong class="text-xs">Model behaviour</strong><span class="text-sm text-muted-foreground">{Option.isSome(snapshot.model_behaviour) ? `${snapshot.model_behaviour.value.settings.length} settings` : "Unavailable"}</span></div>
			<div class="grid gap-1"><strong class="text-xs">Terminal, Git & preview</strong><span class="text-sm text-muted-foreground">Waiting for a selected workspace projection.</span></div>
			{#if Option.isSome(snapshot.error)}
				<p class="flex gap-2 text-sm text-destructive" role="alert"><WifiOff size={16} aria-hidden="true" />{snapshot.error.value}</p>
			{/if}
		</CardContent>
	</Card>
</main>
