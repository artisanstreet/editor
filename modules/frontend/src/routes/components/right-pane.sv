<script lang="ts" effect>
	import { Effect, Option } from "effect";
	import {
		IconActivity as Activity,
		IconLayoutSidebarRightCollapse as CollapseRight,
		IconSettings as Settings,
	} from "@tabler/icons-svelte";

	import type { LiveWorkspaceSnapshot } from "$lib/live-workspace/store";
	import { Button } from "$lib/components/ui/button";
	import { Card, CardContent } from "$lib/components/ui/card";
	import { ScrollArea } from "$lib/components/ui/scroll-area";

	let {
		instance_id,
		live_snapshot,
		on_collapse,
	}: {
		instance_id: string;
		live_snapshot: LiveWorkspaceSnapshot;
		on_collapse?: Effect.Effect<void>;
	} = $props();

	const CollapsePane = Effect.gen(function* () {
		if (on_collapse !== undefined) yield* on_collapse;
	});
</script>

<aside class="right-pane" aria-label="Session">
	<header class="session-header">
		<div><strong>Session</strong><span>{live_snapshot.phase}</span></div>
		<span class="live-badge">Live</span>
		{#if on_collapse}
			<Button variant="ghost" size="icon-sm" class="text-muted-foreground" aria-label="Collapse session pane" title="Collapse session pane" onclick={yield* CollapsePane}>
				<CollapseRight size={17} aria-hidden="true" />
			</Button>
		{/if}
	</header>

	<ScrollArea class="min-h-0 flex-1">
		<div class="session-scroll">
			<Card class="session-card" aria-labelledby={`${instance_id}-session-title`}>
				<CardContent class="p-0">
					<div class="section-title"><Activity size={14} aria-hidden="true" /><h2 id={`${instance_id}-session-title`}>Current session</h2></div>
					<dl class="dense-list">
						<div><dt>Engine</dt><dd>{Option.isSome(live_snapshot.thread_work) ? live_snapshot.thread_work.value.engine_id : "Unavailable"}</dd></div>
						<div><dt>Run</dt><dd class="mono">{Option.isSome(live_snapshot.thread_work) ? live_snapshot.thread_work.value.run_id : "Unavailable"}</dd></div>
						<div><dt>Status</dt><dd>{Option.isSome(live_snapshot.thread_work) ? live_snapshot.thread_work.value.status : "No active work"}</dd></div>
					</dl>
				</CardContent>
			</Card>

			<Card class="session-card" aria-labelledby={`${instance_id}-controls-title`}>
				<CardContent class="p-0">
					<div class="section-title"><Settings size={14} aria-hidden="true" /><h2 id={`${instance_id}-controls-title`}>Controls</h2></div>
					<ul class="row-list">
						<li><span>Guidance</span><strong>{Option.isSome(live_snapshot.global_guidance) ? "Loaded" : "Unavailable"}</strong></li>
						<li><span>Model behaviour</span><strong>{Option.isSome(live_snapshot.model_behaviour) ? `${live_snapshot.model_behaviour.value.settings.length} settings` : "Unavailable"}</strong></li>
					</ul>
				</CardContent>
			</Card>

			<Card class="session-card">
				<CardContent class="unavailable-copy">
					Terminal, Git, previews, usage, and permissions will appear here when their authoritative workspace projections are connected.
				</CardContent>
			</Card>

			{#if Option.isSome(live_snapshot.error)}
				<p class="error-copy" role="alert">{live_snapshot.error.value}</p>
			{/if}
		</div>
	</ScrollArea>
</aside>

<style>
	.right-pane {
		display: flex;
		height: 100%;
		min-height: 0;
		flex-direction: column;
		border: 1px solid var(--line);
		border-radius: var(--radius-lg);
		background: var(--pane);
		overflow: hidden;
	}

	.session-header,
	.session-header > div,
	.section-title,
	.dense-list div,
	.row-list li {
		display: flex;
		align-items: center;
	}

	.session-header {
		min-height: 48px;
		justify-content: space-between;
		gap: 10px;
		padding: 0 12px;
		border-bottom: 1px solid var(--line);
	}

	.session-header > div { gap: 8px; }
	.session-header strong { font-size: 12px; }
	.session-header span,
	.live-badge,
	dt,
	:global(.unavailable-copy) { color: var(--text-muted); font-size: 10px; }
	.live-badge { margin-left: auto; text-transform: uppercase; letter-spacing: 0.07em; }
	.session-scroll { display: grid; align-content: start; padding: 6px 10px; }
	:global(.session-card) { border: 0; border-bottom: 1px solid var(--line); border-radius: 0; background: transparent; }
	.section-title { min-height: 32px; gap: 7px; padding: 0 9px; border-bottom: 1px solid var(--line); }
	h2 { flex: 1; margin: 0; color: var(--text-secondary); font-size: 10px; text-transform: uppercase; letter-spacing: 0.06em; }
	.dense-list, .row-list { margin: 0; padding: 4px 9px; }
	.row-list { list-style: none; }
	.dense-list div, .row-list li { min-height: 28px; justify-content: space-between; gap: 8px; border-bottom: 1px solid var(--line); }
	.dense-list div:last-child, .row-list li:last-child { border-bottom: 0; }
	dd { margin: 0; color: var(--text-secondary); font-size: 10px; text-align: right; }
	.mono { font-family: var(--font-mono); }
	:global(.unavailable-copy) { padding: 12px; line-height: 1.5; }
	.error-copy { margin: 10px; color: var(--destructive); font-size: 11px; }
</style>
