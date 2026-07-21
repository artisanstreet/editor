<script lang="ts" effect>
	import { Effect, Option } from "effect";
	import type { ThreadRetentionPolicy, ThreadSessionPolicy } from "@artisan/protocol";
	import {
		IconActivity as Activity,
		IconBrandGit as GitBranch,
		IconBrowser as Browser,
		IconChevronRight as ChevronRight,
		IconLayoutSidebarRightCollapse as CollapseRight,
		IconPlayerStop as Stop,
		IconRefresh as Refresh,
		IconRotateClockwise as Restart,
		IconSettings as Settings,
		IconShieldCheck as Shield,
		IconTerminal2 as Terminal,
	} from "@tabler/icons-svelte";

	import type {
		LiveWorkspaceSnapshot,
		LiveWorkspaceStore,
	} from "$lib/live-workspace/store";
	import { MakeSessionToolPolicy } from "$lib/live-workspace/session-tool-policy";
	import { Accordion, AccordionContent, AccordionItem, AccordionTrigger } from "$lib/components/ui/accordion";
	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Card, CardContent, CardHeader, CardTitle } from "$lib/components/ui/card";
	import {
		Dialog,
		DialogContent,
		DialogDescription,
		DialogFooter,
		DialogHeader,
		DialogTitle,
	} from "$lib/components/ui/dialog";
	import { Input } from "$lib/components/ui/input";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import {
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
	} from "$lib/components/ui/select";
	import { Switch } from "$lib/components/ui/switch";
	import { Tabs, TabsContent, TabsList, TabsTrigger } from "$lib/components/ui/tabs";
	import { Textarea } from "$lib/components/ui/textarea";

	type WorkspaceController = Pick<
		typeof LiveWorkspaceStore.Service,
		| "Actions"
		| "ExecuteTool"
		| "LoadWorkspaceChangeDiff"
		| "RefreshGitDiff"
		| "RefreshGitWorkspace"
		| "Refresh"
		| "RefreshPreviewTargets"
		| "RefreshTerminals"
		| "WatchTerminalOutput"
		| "RefreshToolApprovals"
		| "RefreshToolInvocations"
		| "RefreshTools"
		| "RefreshWorkspaceChanges"
		| "OpenPreviewInspection"
		| "InspectPreview"
		| "RequestGitIndexMutation"
		| "ReviewWorkspaceChange"
		| "RollbackWorkspaceChange"
		| "ResolveToolApproval"
	>;

	let {
		instance_id,
		live_snapshot,
		controller,
		on_collapse,
	}: {
		instance_id: string;
		live_snapshot: LiveWorkspaceSnapshot;
		controller: WorkspaceController;
		on_collapse?: Effect.Effect<void>;
	} = $props();

	let local_error = $state<string | undefined>();
	let guidance_open = $state(false);
	let guidance_draft = $state("");
	let selected_tab = $state("session");
	let policy_fingerprint = $state("");
	let policy_model = $state("");
	let policy_reasoning = $state<ThreadSessionPolicy["reasoning_effort"]>("medium");
	let policy_permission = $state<ThreadSessionPolicy["permission_mode"]>("on_request");
	let policy_sandbox = $state<ThreadSessionPolicy["sandbox_mode"]>("workspace_write");
	let policy_web_search = $state(false);
	let policy_strict_clarification = $state(false);
	let retention_policy = $state<ThreadRetentionPolicy>();
	let terminal_command = $state("");
	let terminal_input = $state("");
	let preview_url = $state("http://localhost:5173");
	let preview_connector_id = $state("browser");
	let preview_target_id = $state("");

	const selected_thread_id = $derived(Option.getOrUndefined(live_snapshot.selected_thread_id));
	const selected_thread = $derived(
		live_snapshot.threads.find((thread) => thread.thread_id === selected_thread_id),
	);
	const selected_project = $derived(selected_thread?.primary_project);
	const run = $derived(Option.getOrUndefined(live_snapshot.thread_work));
	const session = $derived(Option.getOrUndefined(live_snapshot.session));
	const git = $derived(Option.getOrUndefined(live_snapshot.git_workspace));
	const changes = $derived(Option.getOrUndefined(live_snapshot.workspace_changes));
	const conflicts = $derived(Option.getOrUndefined(live_snapshot.workspace_conflicts));
	const usage = $derived(Option.getOrUndefined(live_snapshot.surface_usage));
	const approvals = $derived(Option.getOrUndefined(live_snapshot.tool_approvals));
	const invocations = $derived(Option.getOrUndefined(live_snapshot.tool_invocations));

	$effect(() => {
		const policy = session?.policy;
		const fingerprint = JSON.stringify([selected_thread_id, policy]);
		if (policy === undefined || fingerprint === policy_fingerprint) return;
		policy_fingerprint = fingerprint;
		policy_model = policy.model ?? "";
		policy_reasoning = policy.reasoning_effort;
		policy_permission = policy.permission_mode;
		policy_sandbox = policy.sandbox_mode;
		policy_web_search = policy.web_search_enabled;
		policy_strict_clarification = policy.strict_clarification;
	});

	const Run = <A>(action: Effect.Effect<A, { readonly message: string }>, after = Effect.void) =>
		action.pipe(
			Effect.flatMap(() => after),
			Effect.matchEffect({
				onFailure: (error) => Effect.sync(() => (local_error = error.message)),
				onSuccess: () => Effect.sync(() => (local_error = undefined)),
			}),
		);

	const CollapsePane = Effect.gen(function* () {
		if (on_collapse !== undefined) yield* on_collapse;
	});

	const RefreshPanel = Effect.gen(function* () {
		if (selected_thread_id === undefined || selected_project === undefined) return;
		const thread_id = selected_thread_id;
		const workspace_id = selected_project.project_id;
		const tool_policy = MakeSessionToolPolicy(session?.policy);
		yield* Effect.all(
			[
				controller.RefreshWorkspaceChanges({ thread_id, workspace_id }),
				controller.RefreshGitWorkspace({ thread_id, workspace_id }),
				controller.RefreshTerminals(thread_id, workspace_id),
				controller.RefreshPreviewTargets(),
				controller.RefreshTools({
					policy: tool_policy,
					thread_id,
					workspace_id,
				}),
				controller.RefreshToolInvocations({ thread_id }),
				controller.RefreshToolApprovals({ thread_id }),
			],
			{ concurrency: "unbounded" },
		);
	});

	const ToggleAutoSteer = Effect.gen(function* () {
		if (selected_thread_id === undefined || session === undefined) return;
		yield* Run(
			controller.Actions.Command({
				thread_id: selected_thread_id,
				payload: { type: "thread.auto_steer.update", enabled: !session.auto_steer_enabled },
			}),
		);
	});

	const TerminalAction = (terminal_id: string, action: "restart" | "stop") =>
		Effect.gen(function* () {
			if (selected_thread_id === undefined) return;
			const invocation_id = `invocation_${globalThis.crypto.randomUUID()}`;
			yield* controller.ExecuteTool({
				invocation_id,
				thread_id: selected_thread_id,
				run_id: run?.run_id,
				agent_id: run?.agent_id,
				input:
					action === "restart"
						? { tool_id: "terminal.restart", terminal_id }
						: { tool_id: "terminal.stop", terminal_id },
				policy: {
					approval: "on_request",
					allow_engine_observation: false,
					allow_git_index_write: false,
					allow_preview_control: false,
					allow_process_control: true,
					allow_workspace_read: true,
					allow_workspace_write: false,
				},
			});
		});

	const SendTerminalCommand = (terminal_id: string, type: "terminal.write" | "terminal.resize" | "terminal.clear" | "terminal.kill" | "terminal.close" | "terminal.restart" | "terminal.pin", pinned?: boolean) =>
		Effect.gen(function* () {
			if (selected_thread_id === undefined) return;
			const payload =
				type === "terminal.write"
					? { type, terminal_id, data: `${terminal_input}\n` }
					: type === "terminal.resize"
						? { type, terminal_id, cols: 120, rows: 32 }
						: type === "terminal.pin"
							? { type, terminal_id, pinned: pinned === true }
							: { type, terminal_id };
			yield* Run(controller.Actions.Command({ thread_id: selected_thread_id, payload }));
			if (type === "terminal.write") terminal_input = "";
		});

	const OpenTerminal = Effect.gen(function* () {
		if (selected_thread_id === undefined || selected_project === undefined) return;
		const [executable, ...args] = terminal_command.trim().split(/\s+/);
		if (executable === undefined || executable.length === 0) {
			local_error = "Enter an executable to open a terminal.";
			return;
		}
		yield* Run(controller.Actions.Command({
			thread_id: selected_thread_id,
			payload: { type: "terminal.open", terminal_id: `terminal_${globalThis.crypto.randomUUID()}`, workspace_id: selected_project.project_id, working_directory: selected_project.root_path, executable, args, cols: 120, rows: 32 },
		}), controller.RefreshTerminals(selected_thread_id, selected_project.project_id));
	});

	const AskAgentToTakeOverTerminal = (terminal_id: string) =>
		Effect.gen(function* () {
			if (selected_thread_id === undefined || run === undefined) return;
			yield* Run(
				controller.Actions.Command({
					thread_id: selected_thread_id,
					payload: {
						type: "run.steer",
						text: `Please take over terminal ${terminal_id} by performing the next terminal action yourself.`,
					},
				}),
			);
		});

	const RegisterPreview = Effect.gen(function* () {
		if (selected_thread_id === undefined || selected_project === undefined) return;
		const url = preview_url.trim();
		const parsed = yield* Effect.option(Effect.try(() => new URL(url)));
		if (Option.isNone(parsed)) {
			local_error = "Enter a valid preview URL.";
			return;
		}
		yield* Run(controller.Actions.RegisterPreviewTarget({ id: preview_target_id.trim() || `preview_${globalThis.crypto.randomUUID()}`, thread_id: selected_thread_id, workspace_id: selected_project.project_id, project_id: selected_project.project_id, url, port: Number(parsed.value.port || (parsed.value.protocol === "https:" ? 443 : 80)), routes: [] }), controller.RefreshPreviewTargets());
	});

	const ResolveApproval = (approval_id: string, invocation_id: string, approved: boolean) =>
		Effect.gen(function* () {
			if (selected_thread_id === undefined) return;
			yield* controller.ResolveToolApproval({
				approval_id,
				approved,
				invocation_id,
				resolution_id: `resolution_${globalThis.crypto.randomUUID()}`,
				thread_id: selected_thread_id,
				run_id: run?.run_id,
				agent_id: run?.agent_id,
			});
		});

	const LoadGitDiff = (scope: "staged" | "unstaged" | "aggregate") =>
		Effect.gen(function* () {
			if (git === undefined || git.workspace.repository_state !== "repository") return;
			yield* controller.RefreshGitDiff({
				expected_snapshot_id: git.workspace.snapshot_id,
				expected_workspace_version: git.workspace.version,
				scope,
				workspace_id: git.workspace.workspace_id,
			});
		});

	const MutateGitPath = (path: string, kind: "stage" | "unstage") =>
		Effect.gen(function* () {
			if (
				selected_thread_id === undefined ||
				git === undefined ||
				git.workspace.repository_state !== "repository"
			)
				return;
			yield* controller.RequestGitIndexMutation({
				expected_snapshot_id: git.workspace.snapshot_id,
				expected_workspace_version: git.workspace.version,
				kind,
				paths: [path],
				thread_id: selected_thread_id,
				workspace_id: git.workspace.workspace_id,
				run_id: run?.run_id,
				agent_id: run?.agent_id,
			});
		});

	const OpenGuidance = Effect.sync(() => {
		guidance_draft = Option.getOrUndefined(live_snapshot.global_guidance)?.content ?? "";
		guidance_open = true;
	});

	const SaveGuidance = Effect.gen(function* () {
		yield* Run(
			controller.Actions.UpdateGlobalGuidance({ content: guidance_draft }),
			controller.Refresh,
		);
		if (local_error === undefined) guidance_open = false;
	});

	const SaveSessionPolicy = Effect.gen(function* () {
		if (selected_thread_id === undefined) return;
		const model = policy_model.trim();
		yield* Run(
			controller.Actions.UpdateThreadSessionPolicy({
				thread_id: selected_thread_id,
				policy: {
					engine_id: "codex",
					...(model.length === 0 ? {} : { model }),
					reasoning_effort: policy_reasoning,
					permission_mode: policy_permission,
					sandbox_mode: policy_sandbox,
					web_search_enabled: policy_web_search,
					strict_clarification: policy_strict_clarification,
				},
			}),
			controller.Refresh,
		);
	});

	const LoadRetentionPolicy = () =>
		controller.Actions.GetThreadRetentionPolicy.pipe(
			Effect.matchEffect({
				onFailure: (error) => Effect.sync(() => (local_error = error.message)),
				onSuccess: (policy) =>
					Effect.sync(() => {
						retention_policy = policy;
						local_error = undefined;
					}),
			}),
		);

	const SaveRetentionPolicy = Effect.gen(function* () {
		if (retention_policy === undefined) return;
		yield* Run(
			controller.Actions.UpdateThreadRetentionPolicy(retention_policy),
			LoadRetentionPolicy(),
		);
	});

	type GuidanceDriftInput = Parameters<
		typeof controller.Actions.ResolveGlobalGuidanceDrift
	>[0];
	type ModelDriftInput = Parameters<typeof controller.Actions.ResolveModelBehaviourDrift>[0];
	const SelectGuidanceCandidate = (
		provider: Parameters<typeof controller.Actions.SelectGlobalGuidance>[0]["provider"],
		content_hash: string,
	) =>
		Run(
			controller.Actions.SelectGlobalGuidance({ provider, content_hash }),
			controller.Refresh,
		);
	const ResolveGuidanceDrift = (
		provider: GuidanceDriftInput["provider"],
		observed_hash: string,
		action: GuidanceDriftInput["action"],
	) =>
		Run(
			controller.Actions.ResolveGlobalGuidanceDrift({ provider, observed_hash, action }),
			controller.Refresh,
		);
	const RetryGuidanceSync = (
		provider: Parameters<typeof controller.Actions.RetryGlobalGuidanceSync>[0]["provider"],
	) =>
		Run(controller.Actions.RetryGlobalGuidanceSync({ provider }), controller.Refresh);
	const UpdateModelSetting = (
		setting_id: Parameters<typeof controller.Actions.UpdateModelBehaviour>[0]["setting_id"],
		value: string,
	) =>
		Run(
			controller.Actions.UpdateModelBehaviour({
				setting_id,
				value:
					value.trim().length === 0
						? { type: "provider_default" }
						: { type: "integer", value: Number(value) },
			}),
			controller.Refresh,
		);
	const ResolveModelDrift = (
		provider_id: string,
		setting_id: ModelDriftInput["setting_id"],
		observed_hash: string,
		action: ModelDriftInput["action"],
	) =>
		Run(
			controller.Actions.ResolveModelBehaviourDrift({
				provider_id,
				setting_id,
				observed_hash,
				action,
			}),
			controller.Refresh,
		);
	const RetryModelSync = (
		provider_id: string,
		setting_id: Parameters<
			typeof controller.Actions.RetryModelBehaviourSync
		>[0]["setting_id"],
	) =>
		Run(
			controller.Actions.RetryModelBehaviourSync({ provider_id, setting_id }),
			controller.Refresh,
		);
</script>

<aside class="flex h-full min-h-0 flex-col overflow-hidden rounded-lg border bg-card" aria-label="Session">
	<header class="flex min-h-12 items-center gap-2 border-b px-3">
		<strong class="text-sm">Session</strong>
		<Badge variant={live_snapshot.phase === "ready" ? "default" : "secondary"}>{live_snapshot.phase}</Badge>
		<Button variant="ghost" size="icon-sm" class="ml-auto" aria-label="Refresh session surfaces" title="Refresh session surfaces" disabled={selected_project === undefined} onclick={yield* RefreshPanel}>
			<Refresh size={16} aria-hidden="true" />
		</Button>
		{#if on_collapse}
			<Button variant="ghost" size="icon-sm" aria-label="Collapse session pane" title="Collapse session pane" onclick={yield* CollapsePane}>
				<CollapseRight size={17} aria-hidden="true" />
			</Button>
		{/if}
	</header>

	<Tabs bind:value={selected_tab} class="min-h-0 flex-1 gap-0">
		<TabsList variant="line" class="w-full justify-start overflow-x-auto rounded-none border-b px-2">
			<TabsTrigger value="session">Session</TabsTrigger>
			<TabsTrigger value="changes">Changes</TabsTrigger>
			<TabsTrigger value="tools">Tools</TabsTrigger>
			<TabsTrigger value="settings">Settings</TabsTrigger>
		</TabsList>
		<ScrollArea class="min-h-0 flex-1">
			<TabsContent value="session" class="m-0 grid gap-3 p-3">
				<Card>
					<CardHeader class="p-3 pb-2"><CardTitle class="flex items-center gap-2 text-sm"><Activity size={15} />Current run</CardTitle></CardHeader>
					<CardContent class="grid gap-2 p-3 pt-0 text-xs">
						<div class="flex justify-between gap-3"><span class="text-muted-foreground">Engine</span><code>{run?.engine_id ?? "Unavailable"}</code></div>
						<div class="flex justify-between gap-3"><span class="text-muted-foreground">Status</span><Badge variant="secondary">{run?.status ?? "idle"}</Badge></div>
						<div class="flex items-center justify-between gap-3"><label for={`${instance_id}-auto-steer`}>Auto-steer follow-ups</label><Switch id={`${instance_id}-auto-steer`} size="sm" checked={session?.auto_steer_enabled ?? false} disabled={session === undefined} onclick={yield* ToggleAutoSteer} /></div>
						{#if session?.latest_intake}<div class="rounded-md border p-2"><strong>Intake:</strong> {session.latest_intake.risk} / {session.latest_intake.resolution}</div>{/if}
					</CardContent>
				</Card>

				<Card>
					<CardHeader class="p-3 pb-2"><CardTitle class="text-sm">Usage and permissions</CardTitle></CardHeader>
					<CardContent class="grid gap-2 p-3 pt-0 text-xs">
						<div class="flex justify-between"><span>Input tokens</span><strong>{usage?.aggregate.input_tokens ?? "Unknown"}</strong></div>
						<div class="flex justify-between"><span>Output tokens</span><strong>{usage?.aggregate.output_tokens ?? "Unknown"}</strong></div>
						<div class="flex justify-between"><span>Pending approvals</span><strong>{approvals?.approvals.filter((approval) => approval.state === "pending").length ?? 0}</strong></div>
					</CardContent>
				</Card>

				<Card>
					<CardHeader class="p-3 pb-2"><CardTitle class="flex items-center gap-2 text-sm"><Browser size={15} />External previews</CardTitle></CardHeader>
					<CardContent class="grid gap-2 p-3 pt-0">
						<div class="grid gap-2 rounded-md border p-2 text-xs">
							<label class="grid gap-1"><span>Local URL</span><Input bind:value={preview_url} aria-label="Local preview URL" /></label>
							<label class="grid gap-1"><span>Target id (optional)</span><Input bind:value={preview_target_id} aria-label="Preview target id" /></label>
							<Button size="xs" onclick={yield* RegisterPreview}>Register local target</Button>
						</div>
						{#each live_snapshot.preview_targets as preview}
							<div class="grid gap-2 rounded-md border p-2 text-xs">
								<div class="flex items-center gap-2"><span class="min-w-0 flex-1 truncate">{preview.url}</span><Badge variant="outline">{preview.state}</Badge></div>
								<div class="flex flex-wrap gap-1">
									<Button size="xs" variant="outline" onclick={yield* Run(controller.Actions.ProbePreviewTarget({ target_id: preview.id }), controller.RefreshPreviewTargets())}>Probe</Button>
									<Button size="xs" onclick={yield* Run(controller.Actions.LaunchPreviewInExternalBrowser({ target_id: preview.id }), controller.RefreshPreviewTargets())}>Open browser</Button>
									<Button size="xs" variant="ghost" onclick={yield* Run(controller.OpenPreviewInspection({ target_id: preview.id, connector_id: preview_connector_id }))}>Inspect</Button>
									<Button size="xs" variant="destructive" onclick={yield* Run(controller.Actions.RemovePreviewTarget({ target_id: preview.id }), controller.RefreshPreviewTargets())}>Remove</Button>
								</div>
							</div>
						{:else}
							<p class="text-xs text-muted-foreground">No local preview target is registered.</p>
						{/each}
						{#if Option.isSome(live_snapshot.preview_inspection_session)}
							<div class="grid gap-2 rounded-md border p-2 text-xs"><div><strong>Inspection</strong> {live_snapshot.preview_inspection_session.value.reconnect_state}</div><div class="flex flex-wrap gap-1"><Button size="xs" variant="outline" onclick={yield* Run(controller.InspectPreview({ session_id: live_snapshot.preview_inspection_session.value.session_id, operation: "health" }))}>Health</Button><Button size="xs" variant="outline" onclick={yield* Run(controller.InspectPreview({ session_id: live_snapshot.preview_inspection_session.value.session_id, operation: "metadata" }))}>Metadata</Button><Button size="xs" variant="ghost" onclick={yield* Run(controller.Actions.ClosePreviewInspectionSession(live_snapshot.preview_inspection_session.value.session_id))}>Close</Button></div></div>
						{/if}
						{#if Option.isSome(live_snapshot.preview_inspection_result)}<pre class="max-h-32 overflow-auto rounded-md bg-muted p-2 text-[10px]">{JSON.stringify(live_snapshot.preview_inspection_result.value, null, 2)}</pre>{/if}
					</CardContent>
				</Card>
			</TabsContent>

			<TabsContent value="changes" class="m-0 grid gap-3 p-3">
				<Card>
					<CardHeader class="p-3 pb-2"><CardTitle class="flex items-center gap-2 text-sm"><GitBranch size={15} />Git workspace</CardTitle></CardHeader>
					<CardContent class="grid gap-2 p-3 pt-0 text-xs">
						{#if git?.workspace.repository_state === "repository"}
							<div class="flex items-center justify-between"><span>{git.workspace.branch.type === "detached" ? "Detached HEAD" : git.workspace.branch.name}</span><Badge variant={git.workspace.clean ? "secondary" : "outline"}>{git.workspace.clean ? "clean" : `${git.workspace.files.length} changed`}</Badge></div>
							<div class="flex gap-1"><Button size="xs" variant="outline" onclick={yield* LoadGitDiff("aggregate")}>View diff</Button><Button size="xs" variant="ghost" onclick={yield* LoadGitDiff("staged")}>Staged</Button><Button size="xs" variant="ghost" onclick={yield* LoadGitDiff("unstaged")}>Unstaged</Button></div>
							{#each git.workspace.files as file}
								<div class="flex items-center gap-2 rounded-md border p-2"><code class="min-w-0 flex-1 truncate">{file.path}</code><Badge variant="outline">{file.porcelain_status}</Badge>{#if file.flags.unstaged}<Button size="xs" variant="outline" onclick={yield* MutateGitPath(file.path, "stage")}>Stage</Button>{/if}{#if file.flags.staged}<Button size="xs" variant="ghost" onclick={yield* MutateGitPath(file.path, "unstage")}>Unstage</Button>{/if}</div>
							{/each}
						{:else}
							<p class="text-muted-foreground">No Git repository projection is available.</p>
						{/if}
						{#if Option.isSome(live_snapshot.git_diff)}<pre class="max-h-56 overflow-auto rounded-md bg-muted p-2 text-[10px]">{live_snapshot.git_diff.value.patch}</pre>{/if}
					</CardContent>
				</Card>

				<Card>
					<CardHeader class="p-3 pb-2"><CardTitle class="text-sm">Attributed changes</CardTitle></CardHeader>
					<CardContent class="grid gap-2 p-3 pt-0 text-xs">
						{#each changes?.changes ?? [] as change}
							<div class="grid gap-2 rounded-md border p-2"><div class="flex items-center gap-2"><code class="min-w-0 flex-1 truncate">{change.path}</code><Badge variant="outline">{change.review_state}</Badge></div><div class="flex flex-wrap gap-1"><Button size="xs" variant="outline" onclick={yield* controller.LoadWorkspaceChangeDiff({ change_id: change.change_id, thread_id: change.thread_id })}>Diff</Button><Button size="xs" onclick={yield* controller.ReviewWorkspaceChange({ change_id: change.change_id, reviewer_kind: "user", outcome: "approved", thread_id: change.thread_id })}>Approve</Button><Button size="xs" variant="secondary" onclick={yield* controller.ReviewWorkspaceChange({ change_id: change.change_id, reviewer_kind: "user", outcome: "changes_requested", thread_id: change.thread_id })}>Request changes</Button><Button size="xs" variant="destructive" disabled={change.rollback_state !== "available"} onclick={yield* controller.RollbackWorkspaceChange({ change_id: change.change_id, expected_after: change.after_identity, thread_id: change.thread_id })}>Rollback</Button></div></div>
						{:else}<p class="text-muted-foreground">No attributed changes.</p>{/each}
						{#if Option.isSome(live_snapshot.workspace_change_diff)}<pre class="max-h-56 overflow-auto rounded-md bg-muted p-2 text-[10px]">{live_snapshot.workspace_change_diff.value.patch}</pre>{/if}
						{#each conflicts?.conflicts ?? [] as conflict}<div class="rounded-md border border-destructive/40 p-2"><strong>Conflict:</strong> {conflict.path} · {conflict.resolution}</div>{/each}
					</CardContent>
				</Card>
			</TabsContent>

			<TabsContent value="tools" class="m-0 grid gap-3 p-3">
				<Card>
					<CardHeader class="p-3 pb-2"><CardTitle class="flex items-center gap-2 text-sm"><Terminal size={15} />Terminals</CardTitle></CardHeader>
					<CardContent class="grid gap-2 p-3 pt-0 text-xs">
						<div class="flex gap-1"><Input bind:value={terminal_command} placeholder="Executable and args" aria-label="Terminal executable" /><Button size="xs" onclick={yield* OpenTerminal}>Open</Button></div>
						{#each live_snapshot.terminals as terminal}
							<div class="grid gap-2 rounded-md border p-2"><div class="flex gap-2"><code class="min-w-0 flex-1 truncate">{terminal.executable} {terminal.args.join(" ")}</code><Badge variant="outline">{terminal.state}</Badge></div><p class="text-muted-foreground">{terminal.working_directory} · {terminal.cols}×{terminal.rows} · PID {terminal.pid ?? "pending"}{terminal.exit_code === undefined ? "" : ` · exit ${terminal.exit_code}`}{terminal.failure ? ` · ${terminal.failure}` : ""}</p><div class="flex gap-1"><Input bind:value={terminal_input} placeholder="Send input" aria-label={`Terminal input ${terminal.terminal_id}`} /><Button size="xs" onclick={yield* SendTerminalCommand(terminal.terminal_id, "terminal.write")}>Send</Button></div><div class="flex flex-wrap gap-1"><Button size="xs" variant="outline" onclick={yield* controller.WatchTerminalOutput(terminal.terminal_id)}>View output</Button><Button size="xs" variant="outline" onclick={yield* SendTerminalCommand(terminal.terminal_id, "terminal.resize")}>120×32</Button><Button size="xs" variant="outline" onclick={yield* SendTerminalCommand(terminal.terminal_id, "terminal.clear")}>Clear</Button><Button size="xs" variant="outline" onclick={yield* SendTerminalCommand(terminal.terminal_id, "terminal.restart")}><Restart size={13} />Restart</Button><Button size="xs" variant="destructive" onclick={yield* SendTerminalCommand(terminal.terminal_id, "terminal.kill")}><Stop size={13} />Kill</Button><Button size="xs" variant="ghost" onclick={yield* SendTerminalCommand(terminal.terminal_id, "terminal.close")}>Close</Button></div>{#if live_snapshot.terminal_output[terminal.terminal_id] !== undefined}<pre class="max-h-40 overflow-auto whitespace-pre-wrap rounded-md bg-muted p-2 text-[10px]" aria-label={`Recent output for ${terminal.terminal_id}`}>{live_snapshot.terminal_output[terminal.terminal_id]}</pre>{/if}</div>
							<div class="flex flex-wrap gap-1 text-xs text-muted-foreground"><span>Owner: {terminal.ownership?.kind === "agent" ? `agent ${terminal.ownership.agent_id}` : "you"}</span>{#each terminal.associated_previews ?? [] as preview}<Badge variant="outline">{preview.port} · {preview.state}</Badge>{/each}<Button size="xs" variant="outline" onclick={yield* SendTerminalCommand(terminal.terminal_id, "terminal.pin", !terminal.pinned)}>{terminal.pinned ? "Unpin" : "Pin"}</Button>{#if run !== undefined}<Button size="xs" variant="ghost" onclick={yield* AskAgentToTakeOverTerminal(terminal.terminal_id)}>Ask agent to take over</Button>{/if}</div>
						{:else}<p class="text-muted-foreground">No terminal sessions.</p>{/each}
					</CardContent>
				</Card>

				<Accordion type="multiple" value={["approvals", "activity"]}>
					<AccordionItem value="approvals"><AccordionTrigger><span class="flex items-center gap-2"><Shield size={15} />Approvals ({approvals?.approvals.length ?? 0})</span></AccordionTrigger><AccordionContent class="grid gap-2">
						{#each approvals?.approvals ?? [] as approval}<div class="grid gap-2 rounded-md border p-2 text-xs"><p>{approval.request.description}</p><div class="flex gap-1"><Badge variant="outline">{approval.state}</Badge>{#if approval.state === "pending"}<Button size="xs" onclick={yield* ResolveApproval(approval.request.approval_id, approval.request.invocation_id, true)}>Approve</Button><Button size="xs" variant="destructive" onclick={yield* ResolveApproval(approval.request.approval_id, approval.request.invocation_id, false)}>Deny</Button>{/if}</div></div>{/each}
					</AccordionContent></AccordionItem>
					<AccordionItem value="activity"><AccordionTrigger>Tool activity ({invocations?.invocations.length ?? 0})</AccordionTrigger><AccordionContent class="grid gap-2">
						{#each invocations?.invocations ?? [] as invocation}<div class="flex items-center gap-2 rounded-md border p-2 text-xs"><code class="min-w-0 flex-1 truncate">{invocation.tool_id}</code><Badge variant="outline">{invocation.lifecycle}</Badge><ChevronRight size={13} /></div>{/each}
					</AccordionContent></AccordionItem>
				</Accordion>
			</TabsContent>

			<TabsContent value="settings" class="m-0 grid gap-3 p-3">
				<Card>
					<CardHeader class="p-3 pb-2"><CardTitle class="text-sm">Session policy</CardTitle></CardHeader>
					<CardContent class="grid gap-3 p-3 pt-0 text-xs">
						<div class="flex items-center justify-between gap-3"><span>Engine</span><Badge variant="secondary">Codex CLI</Badge></div>
						<label class="grid gap-1"><span>Model override</span><Input bind:value={policy_model} placeholder="Codex default" disabled={session === undefined} /></label>
						<label class="grid gap-1"><span>Reasoning effort</span>
							<Select bind:value={policy_reasoning} disabled={session === undefined}>
								<SelectTrigger class="w-full">{policy_reasoning}</SelectTrigger>
								<SelectContent><SelectItem value="low">Low</SelectItem><SelectItem value="medium">Medium</SelectItem><SelectItem value="high">High</SelectItem><SelectItem value="xhigh">Extra high</SelectItem></SelectContent>
							</Select>
						</label>
						<label class="grid gap-1"><span>Permission mode</span>
							<Select bind:value={policy_permission} disabled={session === undefined}>
								<SelectTrigger class="w-full">{policy_permission === "on_request" ? "Ask when needed" : "Never ask"}</SelectTrigger>
								<SelectContent><SelectItem value="on_request">Ask when needed</SelectItem><SelectItem value="never">Never ask</SelectItem></SelectContent>
							</Select>
						</label>
						<label class="grid gap-1"><span>Sandbox</span>
							<Select bind:value={policy_sandbox} disabled={session === undefined}>
								<SelectTrigger class="w-full">{policy_sandbox === "workspace_write" ? "Workspace write" : "Read only"}</SelectTrigger>
								<SelectContent><SelectItem value="workspace_write">Workspace write</SelectItem><SelectItem value="read_only">Read only</SelectItem></SelectContent>
							</Select>
						</label>
						<div class="flex items-center justify-between gap-3"><label for={`${instance_id}-web-search`}>Web search</label><Switch id={`${instance_id}-web-search`} size="sm" bind:checked={policy_web_search} disabled={session === undefined} /></div>
						<div class="flex items-center justify-between gap-3"><label for={`${instance_id}-strict-clarification`}>Strict clarification</label><Switch id={`${instance_id}-strict-clarification`} size="sm" bind:checked={policy_strict_clarification} disabled={session === undefined} /></div>
						<Button size="sm" disabled={session === undefined} onclick={yield* SaveSessionPolicy}>Save session policy</Button>
					</CardContent>
				</Card>
				<Card>
					<CardHeader class="p-3 pb-2"><CardTitle class="text-sm">Thread retention</CardTitle></CardHeader>
					<CardContent class="grid gap-3 p-3 pt-0 text-xs">
						{#if retention_policy === undefined}
							<p class="text-muted-foreground">Load the global inactive-thread cleanup policy.</p>
							<Button size="sm" variant="outline" onclick={yield* LoadRetentionPolicy()}>Load retention policy</Button>
						{:else}
							<div class="flex items-center justify-between gap-3"><label for={`${instance_id}-retention`}>Auto-delete inactive threads</label><Switch id={`${instance_id}-retention`} size="sm" bind:checked={retention_policy.enabled} /></div>
							<label class="grid grid-cols-[1fr_5rem] items-center gap-2"><span>Inactive days</span><Input type="number" min="1" max="3650" bind:value={retention_policy.inactivity_days} /></label>
							<Button size="sm" onclick={yield* SaveRetentionPolicy}>Save retention</Button>
						{/if}
					</CardContent>
				</Card>
				<Card>
					<CardHeader class="p-3 pb-2"><CardTitle class="flex items-center gap-2 text-sm"><Settings size={15} />Global guidance</CardTitle></CardHeader>
					<CardContent class="grid gap-2 p-3 pt-0 text-xs">
						{@const guidance = Option.getOrUndefined(live_snapshot.global_guidance)}
						<p class="line-clamp-4 whitespace-pre-wrap text-muted-foreground">{guidance?.content ?? "Unavailable"}</p>
						<Button size="sm" variant="outline" disabled={guidance === undefined} onclick={yield* OpenGuidance}>Edit guidance</Button>
						{#each guidance?.candidates ?? [] as candidate (candidate.content_hash)}<div class="grid gap-1 rounded-md border p-2"><div class="flex items-center gap-2"><Badge variant="outline">First-run candidate</Badge><span>{candidate.provider}</span></div><p class="line-clamp-2 text-muted-foreground">{candidate.preview}</p><Button class="w-fit" size="xs" onclick={yield* SelectGuidanceCandidate(candidate.provider, candidate.content_hash)}>Use this guidance</Button></div>{/each}
						{#each guidance?.metadata.providers ?? [] as provider (provider.provider)}<div class="grid gap-1 rounded-md border p-2"><div class="flex items-center gap-2"><span>{provider.provider}</span><Badge variant="secondary">{provider.status}</Badge></div>{#if provider.last_error_code}<code class="text-destructive">{provider.last_error_code}</code>{/if}<div class="flex flex-wrap gap-1">{#if provider.status === "drift_detected" && provider.observed_hash}<Button size="xs" variant="outline" onclick={yield* ResolveGuidanceDrift(provider.provider, provider.observed_hash, "import")}>Import</Button><Button size="xs" variant="outline" onclick={yield* ResolveGuidanceDrift(provider.provider, provider.observed_hash, "overwrite")}>Overwrite</Button><Button size="xs" variant="ghost" onclick={yield* ResolveGuidanceDrift(provider.provider, provider.observed_hash, "ignore")}>Ignore</Button>{/if}{#if provider.status === "sync_failed"}<Button size="xs" variant="outline" onclick={yield* RetryGuidanceSync(provider.provider)}>Retry sync</Button>{/if}</div></div>{/each}
					</CardContent>
				</Card>
				<Card>
					<CardHeader class="p-3 pb-2"><CardTitle class="text-sm">Model behaviour</CardTitle></CardHeader>
					<CardContent class="grid gap-2 p-3 pt-0 text-xs">
						{@const model_behaviour = Option.getOrUndefined(live_snapshot.model_behaviour)}
						{#each model_behaviour?.settings ?? [] as setting (setting.setting_id)}
							{@const capability = model_behaviour?.capabilities.find((item) => item.setting_id === setting.setting_id)}
							<div class="grid gap-2 rounded-md border p-2"><label class="grid grid-cols-[1fr_7rem] items-center gap-2"><span>{capability?.display_name ?? setting.setting_id}</span><Input type="number" min={capability?.control.minimum} max={capability?.control.maximum} step={capability?.control.step} value={setting.value.type === "integer" ? setting.value.value : ""} placeholder="Provider default" aria-label={setting.setting_id} onchange={yield* UpdateModelSetting(setting.setting_id, event.currentTarget.value)} /></label><p class="text-muted-foreground">{capability?.description}</p>{#each capability?.provider_support ?? [] as support (support.provider_id)}<div class="flex items-center gap-2"><span>{support.provider_id}</span><Badge variant="outline">{support.state}</Badge><span class="ml-auto text-muted-foreground">{support.activation_timing.replaceAll("_", " ")}</span></div>{/each}</div>
						{/each}
						{#each model_behaviour?.providers ?? [] as provider (`${provider.provider_id}:${provider.setting_id}`)}<div class="grid gap-1 rounded-md border p-2"><div class="flex items-center gap-2"><span>{provider.provider_id}</span><Badge variant="secondary">{provider.status}</Badge></div>{#if provider.last_error_code}<code class="text-destructive">{provider.last_error_code}</code>{/if}<div class="flex flex-wrap gap-1">{#if provider.status === "drift_detected" && provider.observed_hash}<Button size="xs" variant="outline" onclick={yield* ResolveModelDrift(provider.provider_id, provider.setting_id, provider.observed_hash, "import")}>Import</Button><Button size="xs" variant="outline" onclick={yield* ResolveModelDrift(provider.provider_id, provider.setting_id, provider.observed_hash, "overwrite")}>Overwrite</Button><Button size="xs" variant="ghost" onclick={yield* ResolveModelDrift(provider.provider_id, provider.setting_id, provider.observed_hash, "ignore")}>Ignore</Button>{/if}{#if provider.status === "sync_failed"}<Button size="xs" variant="outline" onclick={yield* RetryModelSync(provider.provider_id, provider.setting_id)}>Retry sync</Button>{/if}</div></div>{/each}
					</CardContent>
				</Card>
			</TabsContent>
		</ScrollArea>
	</Tabs>

	{#if local_error !== undefined}<p class="border-t p-2 text-xs text-destructive" role="alert">{local_error}</p>{/if}
	{#if Option.isSome(live_snapshot.error)}<p class="border-t p-2 text-xs text-destructive" role="alert">{live_snapshot.error.value}</p>{/if}
</aside>

<Dialog bind:open={guidance_open}>
	<DialogContent>
		<DialogHeader><DialogTitle>Edit global guidance</DialogTitle><DialogDescription>This canonical guidance is synchronized through the backend.</DialogDescription></DialogHeader>
		<Textarea bind:value={guidance_draft} class="min-h-64 font-mono" aria-label="Global guidance" />
		<DialogFooter><Button variant="outline" onclick={() => (guidance_open = false)}>Cancel</Button><Button onclick={yield* SaveGuidance}>Save</Button></DialogFooter>
	</DialogContent>
</Dialog>
