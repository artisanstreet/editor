<script lang="ts" effect>
	import { Chunk, Effect, Option, Stream } from "effect";
	import type { RichLinkResolution } from "@artisan/protocol";
	import { IconArrowUp as ArrowUp, IconMessageCircle as Message, IconPlayerStop as Stop, IconRoute as Steer } from "@tabler/icons-svelte";
	import type { LiveWorkspaceActions, LiveWorkspaceSnapshot } from "$lib/live-workspace/store";
	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Card, CardContent } from "$lib/components/ui/card";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import { Textarea } from "$lib/components/ui/textarea";
	import ActivityStatus from "./activity-status.sv";

	type RichLinkView = {
		readonly favicon_url?: string;
		readonly resolution: RichLinkResolution;
	};

	let { snapshot, draft = $bindable(), on_send, actions }: { snapshot: LiveWorkspaceSnapshot; draft: string; on_send: (text: string) => Effect.Effect<void>; actions: Pick<LiveWorkspaceActions, "Command" | "OpenAsset" | "ResolveRichLink"> } = $props();
	let action_error = $state<string>();
	let rich_links = $state<Record<string, RichLinkView>>({});
	const rich_link_object_urls = new Set<string>();
	yield* Effect.addFinalizer(
		Effect.sync(() => {
			if (typeof URL.revokeObjectURL !== "function") return;
			for (const url of rich_link_object_urls) URL.revokeObjectURL(url);
		}),
	);
	const thread_id = $derived(Option.getOrUndefined(snapshot.selected_thread_id));
	const run = $derived(Option.getOrUndefined(snapshot.thread_work));
	const selected_thread = $derived(
		snapshot.threads.find((thread) => thread.thread_id === thread_id),
	);
	const entries = $derived(Option.getOrUndefined(snapshot.transcript)?.entries ?? []);
	const can_send = $derived(
		thread_id !== undefined &&
			(run !== undefined ||
				selected_thread?.primary_project !== undefined ||
				Option.getOrUndefined(snapshot.session)?.pending_question?.state === "pending"),
	);
	const pending_engine_question = $derived([...entries].reverse().find((entry) => entry.payload.type === "interaction.question" && entry.payload.state === "requested" && !entries.some((candidate) => candidate.journal_sequence > entry.journal_sequence && candidate.payload.type === "interaction.question" && candidate.payload.question_id === entry.payload.question_id && candidate.payload.state === "resolved"))?.payload);
	const pending_engine_approval = $derived([...entries].reverse().find((entry) => entry.payload.type === "interaction.approval" && entry.payload.state === "requested" && !entries.some((candidate) => candidate.journal_sequence > entry.journal_sequence && candidate.payload.type === "interaction.approval" && candidate.payload.approval_id === entry.payload.approval_id && candidate.payload.state === "resolved"))?.payload);
	const Command = (payload: Parameters<LiveWorkspaceActions["Command"]>[0]["payload"]) => thread_id === undefined ? Effect.void : actions.Command({ ...(run === undefined ? {} : { agent_id: run.agent_id, run_id: run.run_id }), thread_id, payload }).pipe(Effect.matchEffect({ onFailure: (error) => Effect.sync(() => action_error = error.message), onSuccess: () => Effect.sync(() => action_error = undefined) }));
	const Send = Effect.gen(function* () {
		const text = draft.trim(); if (text.length === 0) return;
		const intake = Option.getOrUndefined(snapshot.session)?.pending_question;
		if (intake?.state === "pending") yield* Command({ type: "intake.respond_question", question_id: intake.question_id, answers: { answer: [text] } });
		else if (pending_engine_question?.type === "interaction.question") yield* Command({ type: "run.respond_question", answers: { answer: [text] } });
		else yield* on_send(text);
		if (Option.isNone(snapshot.error) && action_error === undefined) draft = "";
	});
	const Keydown = (event: KeyboardEvent) => Effect.gen(function* () { if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) { event.preventDefault(); yield* Send; } });
	const Copy = (payload: Record<string, unknown>) => "text" in payload ? String(payload.text) : "description" in payload ? String(payload.description) : "assumption" in payload ? String(payload.assumption) : "risk" in payload ? `${String(payload.risk)}: ${String(payload.resolution)}` : "Recorded activity";
	const Links = (text: string) => [...new Set(text.match(/https?:\/\/[^\s<>()]+/gu) ?? [])];
	const ResolveLink = (url: string) =>
		actions.ResolveRichLink({ url }).pipe(
			Effect.flatMap((resolution) =>
				resolution.favicon === undefined
					? Effect.succeed({ resolution } satisfies RichLinkView)
					: actions.OpenAsset(resolution.favicon.asset_id).pipe(
							Effect.flatMap(Stream.runCollect),
							Effect.map((chunks) => {
								const parts = Chunk.toReadonlyArray(chunks);
								const bytes = new Uint8Array(
									parts.reduce((total, part) => total + part.byteLength, 0),
								);
								let offset = 0;
								for (const part of parts) {
									bytes.set(part, offset);
									offset += part.byteLength;
								}
								const favicon_url = URL.createObjectURL(
									new Blob([bytes.slice().buffer], {
										type: resolution.favicon.content_type,
									}),
								);
								rich_link_object_urls.add(favicon_url);
								return { favicon_url, resolution } satisfies RichLinkView;
							}),
						),
			),
			Effect.matchEffect({
				onFailure: (error) => Effect.sync(() => (action_error = error.message)),
				onSuccess: (view) =>
					Effect.sync(() => {
						rich_links = { ...rich_links, [url]: view };
						action_error = undefined;
					}),
			}),
		);
	const SteerRun = Effect.gen(function* () { const text = draft.trim(); if (text.length === 0 || run === undefined) return; yield* Command({ type: "run.steer", text }); if (action_error === undefined) draft = ""; });
	const CancelRun = () => Command({ type: "run.cancel" });
	const ResolveApproval = (approved: boolean) => pending_engine_approval?.type === "interaction.approval" ? Command({ type: "run.respond_approval", approval_id: pending_engine_approval.approval_id, approved }) : Effect.void;
</script>

<section class="grid min-h-0 flex-1 grid-rows-[minmax(0,1fr)_auto] bg-background" aria-label="Chat">
	<ScrollArea class="min-h-0"><div class="mx-auto grid max-w-3xl gap-3 p-4" aria-live="polite">
		{#if Option.isNone(snapshot.transcript)}<p class="text-sm text-muted-foreground">Loading authoritative transcript…</p>
		{:else if snapshot.transcript.value.status === "erased"}<p class="text-sm text-muted-foreground">This transcript was erased by retention policy.</p>
		{:else if snapshot.transcript.value.entries.length === 0}<p class="text-sm text-muted-foreground">No messages yet.</p>
		{:else}{#each snapshot.transcript.value.entries as entry (entry.event_id)}
			{@const entry_text = Copy(entry.payload)}
			<Card class="shadow-none"><CardContent class="grid gap-2 p-3"><div class="flex items-center gap-2"><Message size={14} aria-hidden="true" /><Badge variant="outline">{entry.payload.type}</Badge><time class="ml-auto text-xs text-muted-foreground">{entry.occurred_at}</time></div><p class="whitespace-pre-wrap text-sm">{entry_text}</p>
				{#each Links(entry_text) as url (url)}
					{@const link = rich_links[url]}
					{#if link === undefined}
						<Button class="w-fit" size="xs" variant="outline" onclick={yield* ResolveLink(url)}>Preview link</Button>
					{:else}
						<div class="flex items-center gap-3 rounded-md border bg-muted/30 p-2">
							{#if link.favicon_url}<img class="size-5 rounded-sm" src={link.favicon_url} alt="" />{/if}
							<div class="min-w-0"><strong class="block truncate text-xs">{link.resolution.title ?? link.resolution.page_name}</strong><span class="block truncate text-xs text-muted-foreground">{link.resolution.site_name} · {link.resolution.final_url}</span></div>
						</div>
					{/if}
				{/each}
			</CardContent></Card>
		{/each}{/if}
		{#if Option.isSome(snapshot.session) && snapshot.session.value.pending_question?.state === "pending"}<Card class="border-primary"><CardContent class="p-3"><Badge>Clarification needed</Badge><p class="mt-2 text-sm">{snapshot.session.value.pending_question.text}</p></CardContent></Card>{/if}
		{#if pending_engine_question?.type === "interaction.question"}<Card class="border-primary"><CardContent class="p-3"><Badge>Agent question</Badge><p class="mt-2 text-sm">{pending_engine_question.text}</p></CardContent></Card>{/if}
		{#if pending_engine_approval?.type === "interaction.approval"}<Card class="border-primary"><CardContent class="grid gap-2 p-3"><Badge>Approval required</Badge><p class="text-sm">{pending_engine_approval.description}</p><div class="flex gap-2"><Button size="sm" onclick={yield* ResolveApproval(true)}>Approve</Button><Button size="sm" variant="destructive" onclick={yield* ResolveApproval(false)}>Deny</Button></div></CardContent></Card>{/if}
	</div></ScrollArea>
	<div class="grid gap-2 border-t bg-card p-3"><ActivityStatus {snapshot} /><div class="mx-auto flex w-full max-w-3xl items-end gap-2"><Textarea bind:value={draft} class="min-h-20" aria-label="Message Codex" placeholder={Option.isSome(snapshot.session) && snapshot.session.value.pending_question?.state === "pending" || pending_engine_question?.type === "interaction.question" ? "Answer the clarification" : run === undefined ? "Start a Codex run" : "Message the active Codex run"} onkeydown={yield* Keydown(event)} /><Button size="icon" aria-label="Send message" disabled={draft.trim().length === 0 || !can_send} onclick={yield* Send}><ArrowUp size={16} /></Button></div><div class="mx-auto flex w-full max-w-3xl items-center gap-2">{#if run?.status === "running" || run?.status === "waiting"}<Button size="xs" variant="outline" disabled={draft.trim().length === 0} onclick={yield* SteerRun}><Steer size={13} />Steer now</Button><Button size="xs" variant="destructive" onclick={yield* CancelRun()}><Stop size={13} />Cancel run</Button>{/if}<p class="ml-auto text-xs text-muted-foreground">{Option.getOrUndefined(snapshot.session)?.auto_steer_enabled ? "Follow-ups auto-steer when supported." : "Follow-ups are queued."}</p></div>{#if action_error}<p class="mx-auto mt-2 max-w-3xl text-xs text-destructive" role="alert">{action_error}</p>{/if}</div>
</section>
