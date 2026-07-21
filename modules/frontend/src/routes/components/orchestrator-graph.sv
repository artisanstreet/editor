<script lang="ts" effect>
	import { Effect, Option } from "effect";
	import { IconGitMerge as Graph, IconPlus as Plus, IconRobot as Robot } from "@tabler/icons-svelte";
	import type { LiveWorkspaceActions, LiveWorkspaceSnapshot } from "$lib/live-workspace/store";
	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Card, CardContent, CardHeader, CardTitle } from "$lib/components/ui/card";
	import { Input } from "$lib/components/ui/input";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import { Textarea } from "$lib/components/ui/textarea";

	let {
		snapshot,
		on_select_group,
		actions,
	}: {
		snapshot: LiveWorkspaceSnapshot;
		on_select_group: (group_id: string) => Effect.Effect<void>;
		actions: Pick<LiveWorkspaceActions, "Command">;
	} = $props();
	let steer_text = $state<Record<string, string>>({});
	let agent_names = $state<Record<string, string>>({});
	let first_assignment = $state("");
	let second_assignment = $state("");
	let action_error = $state<string>();
	const thread_id = $derived(Option.getOrUndefined(snapshot.selected_thread_id));
	const run = $derived(Option.getOrUndefined(snapshot.thread_work));
	const session = $derived(Option.getOrUndefined(snapshot.session));
	const selected_thread = $derived(
		snapshot.threads.find((thread) => thread.thread_id === thread_id),
	);
	const project = $derived(selected_thread?.primary_project);
	const Command = (payload: Parameters<LiveWorkspaceActions["Command"]>[0]["payload"]) =>
		thread_id === undefined
			? Effect.void
			: actions
					.Command({
						...(run === undefined ? {} : { agent_id: run.agent_id, run_id: run.run_id }),
						thread_id,
						payload,
					})
					.pipe(
						Effect.matchEffect({
							onFailure: (error) => Effect.sync(() => (action_error = error.message)),
							onSuccess: () => Effect.sync(() => (action_error = undefined)),
						}),
					);
	const Pause = (group_id: string, assignment_id: string) =>
		Command({ type: "assignment.pause", group_id, assignment_id });
	const Resume = (group_id: string, assignment_id: string) =>
		Command({ type: "assignment.resume", group_id, assignment_id });
	const Stop = (group_id: string, assignment_id: string) =>
		Command({ type: "assignment.stop", group_id, assignment_id });
	const Retry = (group_id: string, assignment_id: string) =>
		Command({ type: "assignment.retry", group_id, assignment_id });
	const SteerAssignment = (group_id: string, assignment_id: string) =>
		Effect.gen(function* () {
			const text = steer_text[assignment_id]?.trim();
			if (!text) return;
			yield* Command({ type: "assignment.steer", group_id, assignment_id, text });
			if (action_error === undefined)
				steer_text = { ...steer_text, [assignment_id]: "" };
		});
	const RenameAgent = (group_id: string, agent_id: string, current_name: string) =>
		Effect.gen(function* () {
			const display_name = (agent_names[agent_id] ?? current_name).trim();
			if (display_name.length === 0) return;
			yield* Command({ type: "agent_instance.rename", group_id, agent_id, display_name });
		});
	const StartGroup = Effect.gen(function* () {
		const first = first_assignment.trim();
		const second = second_assignment.trim();
		if (thread_id === undefined || project === undefined || first.length === 0 || second.length === 0)
			return;
		const group_id = `group_${globalThis.crypto.randomUUID()}`;
		const parent_node_id = run?.agent_id ?? `coordinator_${globalThis.crypto.randomUUID()}`;
		const MakeAssignment = (index: number, instructions: string) => ({
			assignment_id: `assignment_${globalThis.crypto.randomUUID()}`,
			display_name: index === 1 ? "Juniper" : "Mosaic",
			role: "worker",
			scope: { kind: "repo" as const, value: project.root_path, write_access: true },
			engine_id: "codex",
			profile: session?.policy.model ?? "default",
			workspace: {
				workspace_id: project.project_id,
				working_directory: project.root_path,
				isolation: "shared" as const,
			},
			permission_policy: {
				approval: session?.policy.permission_mode === "never" ? ("never" as const) : ("on_request" as const),
				network_access: session?.policy.web_search_enabled ?? false,
				write_access: session?.policy.sandbox_mode !== "read_only",
			},
			summary_contract: "Return a concise result with evidence and remaining risks.",
			parent_node_id,
			expected_result: instructions,
			instructions,
			max_attempts: 2,
		});
		yield* Command({
			type: "orchestration.group.start",
			group_id,
			assignments: [MakeAssignment(1, first), MakeAssignment(2, second)],
			max_concurrency: 2,
			name_bank: ["Juniper", "Mosaic", "Nimbus", "Quartz"],
		});
		if (action_error === undefined) {
			first_assignment = "";
			second_assignment = "";
		}
	});
</script>

<section class="grid min-h-0 flex-1 grid-cols-[14rem_minmax(0,1fr)] bg-background" aria-label="Orchestrator">
	<nav class="border-r p-2" aria-label="Orchestration groups">
		<div class="grid gap-1">
			{#if Option.isSome(snapshot.orchestration_groups)}
				{#each snapshot.orchestration_groups.value.groups as group (group.group_id)}
					<Button class="h-auto justify-start" variant={Option.getOrUndefined(snapshot.selected_group_id) === group.group_id ? "secondary" : "ghost"} onclick={yield* on_select_group(group.group_id)}>
						<span class="grid text-left"><span>{group.group_id}</span><small class="text-muted-foreground">{group.state}</small></span>
					</Button>
				{/each}
			{:else}
				<p class="p-2 text-xs text-muted-foreground">Loading groups…</p>
			{/if}
		</div>
	</nav>
	<ScrollArea class="min-h-0">
		<div class="grid gap-4 p-4">
			<Card>
				<CardHeader class="pb-2"><CardTitle class="text-sm">Start a Codex fan-out group</CardTitle></CardHeader>
				<CardContent class="grid gap-2">
					<div class="grid gap-2 md:grid-cols-2"><Textarea bind:value={first_assignment} placeholder="First bounded assignment" aria-label="First orchestration assignment" /><Textarea bind:value={second_assignment} placeholder="Second bounded assignment" aria-label="Second orchestration assignment" /></div>
					<Button class="w-fit" size="sm" disabled={project === undefined || first_assignment.trim().length === 0 || second_assignment.trim().length === 0} onclick={yield* StartGroup}><Plus size={14} />Start group</Button>
				</CardContent>
			</Card>
			{#if Option.isNone(snapshot.orchestration_graph)}
				<p class="text-sm text-muted-foreground">Select a group to inspect its authoritative graph.</p>
			{:else}
				{@const graph = snapshot.orchestration_graph.value}
				<header class="flex items-center gap-2"><Graph size={18} /><h1 class="font-semibold">{graph.group.group_id}</h1><Badge variant="outline">{graph.group.state}</Badge></header>
				<div class="grid gap-3 md:grid-cols-2">
					{#each graph.assignments as assignment (assignment.assignment_id)}
						{@const agent = graph.agent_instances.find((item) => item.agent_id === assignment.agent_id)}
						<Card>
							<CardHeader class="pb-2"><CardTitle class="flex items-center gap-2 text-sm"><Robot size={15} />{agent?.display_name ?? assignment.agent_id}<Badge variant="outline">{assignment.state}</Badge></CardTitle></CardHeader>
							<CardContent class="grid gap-2 text-xs text-muted-foreground">
								<p>{assignment.heartbeat?.short_description ?? assignment.expected_result}</p>
								<p>{assignment.role} · attempt {assignment.current_attempt}/{assignment.max_attempts}</p>
								{#if assignment.heartbeat?.blocked_reason}<p class="text-destructive">{assignment.heartbeat.blocked_reason}</p>{/if}
								<div class="flex flex-wrap gap-1">
									{#if assignment.state === "running"}<Button size="xs" variant="outline" onclick={yield* Pause(graph.group.group_id, assignment.assignment_id)}>Pause</Button><Button size="xs" variant="destructive" onclick={yield* Stop(graph.group.group_id, assignment.assignment_id)}>Stop</Button>
									{:else if assignment.state === "waiting" || assignment.state === "blocked"}<Button size="xs" variant="outline" onclick={yield* Resume(graph.group.group_id, assignment.assignment_id)}>Resume</Button>
									{:else if assignment.state === "failed" || assignment.state === "stopped"}<Button size="xs" variant="outline" onclick={yield* Retry(graph.group.group_id, assignment.assignment_id)}>Retry</Button>{/if}
								</div>
								{#if assignment.state === "running" || assignment.state === "waiting"}<div class="flex gap-1"><Input value={steer_text[assignment.assignment_id] ?? ""} placeholder="Steer assignment" aria-label={`Steer ${agent?.display_name ?? assignment.assignment_id}`} oninput={steer_text = { ...steer_text, [assignment.assignment_id]: event.currentTarget.value }} /><Button size="sm" variant="secondary" disabled={!steer_text[assignment.assignment_id]?.trim()} onclick={yield* SteerAssignment(graph.group.group_id, assignment.assignment_id)}>Send</Button></div>{/if}
								{#if agent}<div class="flex gap-1"><Input value={agent_names[agent.agent_id] ?? agent.display_name} aria-label={`Rename ${agent.display_name}`} oninput={agent_names = { ...agent_names, [agent.agent_id]: event.currentTarget.value }} /><Button size="sm" variant="outline" onclick={yield* RenameAgent(graph.group.group_id, agent.agent_id, agent.display_name)}>Rename</Button></div>{/if}
							</CardContent>
						</Card>
					{/each}
				</div>
				<div class="grid gap-2 md:grid-cols-3"><Card><CardContent class="p-3 text-sm">{graph.agent_runs.length} runs</CardContent></Card><Card><CardContent class="p-3 text-sm">{graph.joins.length} joins</CardContent></Card><Card><CardContent class="p-3 text-sm">{graph.artifacts.length} artifacts</CardContent></Card></div>
				{#if graph.joins.length > 0}<Card><CardHeader class="pb-2"><CardTitle class="text-sm">Joins</CardTitle></CardHeader><CardContent class="grid gap-2">{#each graph.joins as join (join.join_id)}<div class="flex items-center gap-2 rounded-md border p-2 text-xs"><Badge variant="outline">{join.state}</Badge><span>{join.strategy}</span><span class="ml-auto text-muted-foreground">{join.upstream_assignment_ids.length} upstream</span></div>{/each}</CardContent></Card>{/if}
				{#if graph.artifacts.length > 0}<Card><CardHeader class="pb-2"><CardTitle class="text-sm">Artifacts and summaries</CardTitle></CardHeader><CardContent class="grid gap-2">{#each graph.artifacts as artifact (artifact.artifact_id)}<div class="grid gap-1 rounded-md border p-2 text-xs"><div class="flex items-center gap-2"><Badge variant="outline">{artifact.kind}</Badge><strong>{artifact.label}</strong></div>{#if artifact.content}<p class="whitespace-pre-wrap text-muted-foreground">{artifact.content}</p>{/if}{#if artifact.uri}<code class="truncate">{artifact.uri}</code>{/if}</div>{/each}</CardContent></Card>{/if}
				{#if action_error}<p class="text-sm text-destructive" role="alert">{action_error}</p>{/if}
			{/if}
		</div>
	</ScrollArea>
</section>
