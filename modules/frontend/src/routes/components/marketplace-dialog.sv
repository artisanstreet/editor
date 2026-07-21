<script lang="ts" effect>
	import { Effect, Option } from "effect";
	import type {
		CapabilityConnectPreview,
		CapabilityConnectPreviewRequest,
		MarketplaceScope,
		RoutineInstallPreview,
		RoutineInstallPreviewRequest,
	} from "@artisan/protocol";
	import {
		IconBolt as Bolt,
		IconCheck as Check,
		IconCloud as Cloud,
		IconKey as Key,
		IconPlug as Plug,
		IconRefresh as Refresh,
		IconRotateClockwise as Restart,
		IconShieldCheck as ShieldCheck,
		IconTrash as Trash,
	} from "@tabler/icons-svelte";

	import type { LiveWorkspaceActions, LiveWorkspaceSnapshot } from "$lib/live-workspace/store";
	import { Accordion, AccordionContent, AccordionItem, AccordionTrigger } from "$lib/components/ui/accordion";
	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Card, CardContent, CardHeader, CardTitle } from "$lib/components/ui/card";
	import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "$lib/components/ui/dialog";
	import { Input } from "$lib/components/ui/input";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import { Select, SelectContent, SelectItem, SelectTrigger } from "$lib/components/ui/select";
	import { Tabs, TabsContent, TabsList, TabsTrigger } from "$lib/components/ui/tabs";
	import { Textarea } from "$lib/components/ui/textarea";
	import MarketplaceList from "./marketplace-list.sv";

	type MarketplaceActions = Pick<
		LiveWorkspaceActions,
		| "PreviewRoutineInstall" | "RequestRoutineInstall" | "DecideRoutineInstall" | "EnableRoutine" | "DisableRoutine" | "RemoveRoutine" | "RollbackRoutine" | "SyncRoutine" | "ResolveRoutineDrift" | "RequestRoutineDriftOverwrite" | "DecideRoutineDriftOverwrite" | "InvokeRoutine" | "DiscoverNpxSkills" | "ImportNpxSkills"
		| "PreviewCapabilityConnect" | "RequestCapabilityConnect" | "DecideCapabilityConnect" | "StartCapability" | "ReconnectCapability" | "CheckCapabilityHealth" | "DisconnectCapability" | "RestartCapability" | "UninstallCapability" | "EnableCapability" | "DisableCapability" | "RemoveCapability" | "SyncCapability" | "ResolveCapabilityDrift" | "RequestCapabilityDriftOverwrite" | "DecideCapabilityDriftOverwrite" | "RequestCapabilityInvocation" | "DecideCapabilityInvocation" | "InvokeCapability" | "GetCapabilityOAuthStatus" | "BeginCapabilityOAuth" | "CompleteCapabilityOAuth" | "RefreshCapabilityOAuth" | "RevokeCapabilityOAuth"
	>;

	export interface MarketplaceApi {
		readonly Actions: MarketplaceActions;
		readonly GetCapabilityDetail: (input: { readonly capability_id: string; readonly scope: MarketplaceScope }) => Effect.Effect<void>;
		readonly GetRoutineDetail: (input: { readonly routine_id: string; readonly scope: MarketplaceScope }) => Effect.Effect<void>;
		readonly RefreshMarketplace: (input: { readonly category?: "routine" | "capability"; readonly text?: string }) => Effect.Effect<void>;
	}

	let { api, live_snapshot, open = $bindable(false) }: { api?: MarketplaceApi; live_snapshot: LiveWorkspaceSnapshot; open?: boolean } = $props();
	let category = $state<"all" | "routine" | "capability">("all");
	let query = $state("");
	let action_error = $state<string>();
	let routine_source = $state<RoutineInstallPreviewRequest["source"]["kind"]>("local");
	let routine_locator = $state("");
	let routine_scope_kind = $state<MarketplaceScope["kind"]>("global");
	let routine_scope_id = $state("");
	let routine_preview = $state<RoutineInstallPreview>();
	let routine_approval_id = $state("");
	let npx_package = $state("");
	let npx_candidates = $state<ReadonlyArray<{ readonly name: string; readonly preview_fingerprint?: string }>>([]);
	let capability_source = $state<CapabilityConnectPreviewRequest["source"]["kind"]>("local");
	let capability_locator = $state("");
	let capability_scope_kind = $state<MarketplaceScope["kind"]>("global");
	let capability_scope_id = $state("");
	let capability_transport = $state<"stdio" | "streamable_http">("stdio");
	let capability_command = $state("");
	let capability_args = $state("");
	let capability_url = $state("");
	let capability_preview = $state<CapabilityConnectPreview>();
	let capability_approval_id = $state("");
	let oauth_callback = $state("");
	let oauth_authorization_url = $state("");
	let oauth_continuation_reference = $state("");
	let invocation_tool = $state("");
	let invocation_args = $state("{}");
	let invocation_approval = $state("");
	let invocation_fingerprint = $state("");
	let drift_engine = $state("codex");
	let drift_revision = $state("");
	let drift_approval = $state("");
	let drift_fingerprint = $state("");
	let routine_task = $state("");
	let routine_command = $state("");

	const Scope = (kind: MarketplaceScope["kind"], id: string): MarketplaceScope => kind === "global" ? { kind } : kind === "workspace" ? { kind, workspace_id: id } : { kind, project_id: id };
	const Id = () => crypto.randomUUID();
	const Failure = (error: unknown) => error instanceof Error ? error.message : String(error);
	const CaptureOAuthHandoff = (value: unknown) => {
		if (typeof value !== "object" || value === null) return;
		const candidate = value as Record<string, unknown>;
		if (typeof candidate.authorization_url !== "string" || typeof candidate.continuation_reference !== "string") return;
		oauth_authorization_url = candidate.authorization_url;
		oauth_continuation_reference = candidate.continuation_reference;
	};
	const Run = (operation: Effect.Effect<unknown>) => operation.pipe(Effect.matchEffect({ onFailure: (error) => Effect.sync(() => { action_error = Failure(error); }), onSuccess: (value) => Effect.sync(() => { CaptureOAuthHandoff(value); action_error = undefined; }) }));
	const RefreshMarketplace = () => api === undefined ? Effect.void : Run(api.RefreshMarketplace(category === "all" ? {} : { category, text: query || undefined }));
	const After = (operation: Effect.Effect<unknown>) => Effect.gen(function* () { yield* Run(operation); yield* RefreshMarketplace(); });
	const RoutineScope = () => Scope(routine_scope_kind, routine_scope_id);
	const CapabilityScope = () => Scope(capability_scope_kind, capability_scope_id);
	const SelectRoutine = (routine_id: string, scope: MarketplaceScope) => api === undefined ? Effect.void : Run(api.GetRoutineDetail({ routine_id, scope }));
	const SelectCapability = (capability_id: string, scope: MarketplaceScope) => api === undefined ? Effect.void : Effect.gen(function* () { yield* Run(api.GetCapabilityDetail({ capability_id, scope })); yield* Run(api.Actions.GetCapabilityOAuthStatus({ capability_id, scope })); });
	const PreviewRoutine = () => api === undefined ? Effect.void : api.Actions.PreviewRoutineInstall({ scope: RoutineScope(), source: { kind: routine_source, locator: routine_locator } }).pipe(Effect.matchEffect({ onFailure: (error) => Effect.sync(() => { action_error = Failure(error); }), onSuccess: (value) => Effect.sync(() => { routine_preview = value; routine_approval_id = Id(); action_error = undefined; }) }));
	const RequestRoutine = () => api === undefined || routine_preview === undefined ? Effect.void : After(api.Actions.RequestRoutineInstall({ approval_id: routine_approval_id, preview_fingerprint: routine_preview.preview_fingerprint, requested_by: "user", scope: routine_preview.scope, source: routine_preview.source }));
	const PreviewCapability = () => {
		if (api === undefined) return Effect.void;
		const transport: CapabilityConnectPreviewRequest["transport"] = capability_transport === "stdio" ? { kind: "stdio", command: capability_command, args: capability_args ? capability_args.split(" ").filter(Boolean) : [], startup_timeout_ms: 10_000 } : { kind: "streamable_http", url: capability_url };
		return api.Actions.PreviewCapabilityConnect({ scope: CapabilityScope(), source: { kind: capability_source, locator: capability_locator }, transport, auth: { kind: "none" } }).pipe(Effect.matchEffect({ onFailure: (error) => Effect.sync(() => { action_error = Failure(error); }), onSuccess: (value) => Effect.sync(() => { capability_preview = value; capability_approval_id = Id(); action_error = undefined; }) }));
	};
	const RequestCapability = () => api === undefined || capability_preview === undefined ? Effect.void : After(api.Actions.RequestCapabilityConnect({ approval_id: capability_approval_id, preview_fingerprint: capability_preview.preview_fingerprint, requested_by: "user", scope: capability_preview.scope, source: capability_preview.source, transport: capability_preview.transport, auth: capability_preview.auth }));
	const RoutineDetail = $derived(Option.getOrUndefined(live_snapshot.routine_detail));
	const CapabilityDetail = $derived(Option.getOrUndefined(live_snapshot.capability_detail));
	const OAuth = $derived(Option.getOrUndefined(live_snapshot.capability_oauth));
	const CopyOAuthUrl = () =>
		oauth_authorization_url.length === 0
			? Effect.void
			: Effect.tryPromise(() => navigator.clipboard.writeText(oauth_authorization_url)).pipe(
					Effect.matchEffect({
						onFailure: (error) =>
							Effect.sync(() => {
								action_error = Failure(error);
							}),
						onSuccess: () => Effect.sync(() => (action_error = undefined)),
					}),
				);
</script>

<Dialog bind:open>
	<DialogContent class="flex h-[min(52rem,calc(100dvh-2rem))] max-w-6xl flex-col gap-3 p-4">
		<DialogHeader class="pr-8"><DialogTitle>Marketplace</DialogTitle><DialogDescription>Install routines and connect capabilities through Artisan’s approval-bound control plane. Codex is the only active engine.</DialogDescription></DialogHeader>
		<div class="flex flex-wrap gap-2"><Input bind:value={query} class="min-w-48 flex-1" placeholder="Search installed items" aria-label="Search Marketplace" onkeydown={event.key === "Enter" ? yield* RefreshMarketplace() : undefined} /><Button variant="outline" onclick={yield* RefreshMarketplace()}><Refresh size={15} />Refresh</Button></div>
		{#if action_error}<p class="text-destructive text-sm" role="alert">{action_error}</p>{/if}
		<Tabs bind:value={category} class="min-h-0 flex-1"><TabsList aria-label="Marketplace categories"><TabsTrigger value="all">All</TabsTrigger><TabsTrigger value="routine"><Bolt size={14} />Routines</TabsTrigger><TabsTrigger value="capability"><Plug size={14} />Capabilities</TabsTrigger></TabsList>
			<TabsContent value="all" class="min-h-0"><MarketplaceList {live_snapshot} {query} show_routines show_capabilities {SelectRoutine} {SelectCapability} /></TabsContent>
			<TabsContent value="routine" class="min-h-0"><MarketplaceList {live_snapshot} {query} show_routines show_capabilities={false} {SelectRoutine} {SelectCapability} /></TabsContent>
			<TabsContent value="capability" class="min-h-0"><MarketplaceList {live_snapshot} {query} show_routines={false} show_capabilities {SelectRoutine} {SelectCapability} /></TabsContent>
		</Tabs>
		<ScrollArea class="max-h-80 rounded-md border"><div class="grid gap-3 p-3">
			<Card><CardHeader><CardTitle class="text-sm">Install a routine</CardTitle></CardHeader><CardContent class="grid gap-2"><div class="grid gap-2 sm:grid-cols-4"><Select bind:value={routine_source}><SelectTrigger class="w-full">{routine_source.replaceAll("_", " ")}</SelectTrigger><SelectContent><SelectItem value="local">Local</SelectItem><SelectItem value="git">Git</SelectItem><SelectItem value="package_manager">Package manager</SelectItem><SelectItem value="catalog">Catalog</SelectItem><SelectItem value="provider_import">Provider import</SelectItem><SelectItem value="plugin_bundle">Plugin bundle</SelectItem></SelectContent></Select><Input bind:value={routine_locator} aria-label="Routine source locator" placeholder="Source locator" /><Select bind:value={routine_scope_kind}><SelectTrigger class="w-full">{routine_scope_kind}</SelectTrigger><SelectContent><SelectItem value="global">Global</SelectItem><SelectItem value="workspace">Workspace</SelectItem><SelectItem value="project">Project</SelectItem></SelectContent></Select><Input bind:value={routine_scope_id} aria-label="Routine scope identifier" placeholder="Scope id (not global)" /></div><div class="flex flex-wrap gap-2"><Button size="sm" variant="outline" onclick={yield* PreviewRoutine()}><ShieldCheck size={14} />Preview install</Button>{#if routine_preview}<Badge variant="secondary">{routine_preview.candidate_name} · {routine_preview.trust}</Badge><Button size="sm" onclick={yield* RequestRoutine()}>Request approval</Button><Button size="sm" variant="outline" onclick={yield* After(api?.Actions.DecideRoutineInstall({ approval_id: routine_approval_id, preview_fingerprint: routine_preview.preview_fingerprint, approved: true }) ?? Effect.void)}><Check size={14} />Approve</Button><Button size="sm" variant="ghost" onclick={yield* After(api?.Actions.DecideRoutineInstall({ approval_id: routine_approval_id, preview_fingerprint: routine_preview.preview_fingerprint, approved: false }) ?? Effect.void)}>Deny</Button>{/if}</div>{#if routine_preview}<p class="text-muted-foreground text-xs">Files: {routine_preview.files.map((file) => file.path).join(", ") || "none"}. Permissions: {routine_preview.permissions.map((item) => item.kind).join(", ") || "none"}.</p>{/if}</CardContent></Card>
			<Card><CardHeader><CardTitle class="text-sm">Import npx skills</CardTitle></CardHeader><CardContent class="flex flex-wrap gap-2"><Input bind:value={npx_package} class="min-w-52 flex-1" placeholder="Package spec" /><Button size="sm" variant="outline" onclick={yield* (api === undefined ? Effect.void : api.Actions.DiscoverNpxSkills({ package_spec: npx_package, scope: RoutineScope() }).pipe(Effect.matchEffect({ onFailure: (error) => Effect.sync(() => { action_error = Failure(error); }), onSuccess: (value) => Effect.sync(() => { npx_candidates = value.candidates; }) })))} >Discover</Button>{#each npx_candidates as candidate (candidate.name)}<Button size="sm" onclick={yield* After(api?.Actions.ImportNpxSkills({ candidate_name: candidate.name, package_spec: npx_package, preview_fingerprint: candidate.preview_fingerprint ?? "", scope: RoutineScope() }) ?? Effect.void)}>Import {candidate.name}</Button>{/each}</CardContent></Card>
			<Card><CardHeader><CardTitle class="text-sm">Connect a capability</CardTitle></CardHeader><CardContent class="grid gap-2"><div class="grid gap-2 sm:grid-cols-3"><Select bind:value={capability_source}><SelectTrigger class="w-full">{capability_source.replaceAll("_", " ")}</SelectTrigger><SelectContent><SelectItem value="local">Local</SelectItem><SelectItem value="git">Git</SelectItem><SelectItem value="package_manager">Package manager</SelectItem><SelectItem value="catalog">Catalog</SelectItem><SelectItem value="provider_import">Provider import</SelectItem><SelectItem value="plugin_bundle">Plugin bundle</SelectItem></SelectContent></Select><Input bind:value={capability_locator} aria-label="Capability source locator" placeholder="Source locator" /><Select bind:value={capability_scope_kind}><SelectTrigger class="w-full">{capability_scope_kind}</SelectTrigger><SelectContent><SelectItem value="global">Global</SelectItem><SelectItem value="workspace">Workspace</SelectItem><SelectItem value="project">Project</SelectItem></SelectContent></Select><Input bind:value={capability_scope_id} aria-label="Capability scope identifier" placeholder="Scope id (not global)" /><Select bind:value={capability_transport}><SelectTrigger class="w-full">{capability_transport === "stdio" ? "stdio" : "Streamable HTTP"}</SelectTrigger><SelectContent><SelectItem value="stdio">stdio</SelectItem><SelectItem value="streamable_http">Streamable HTTP</SelectItem></SelectContent></Select></div><div class="grid gap-2 sm:grid-cols-2"><Input bind:value={capability_command} aria-label="Capability command" placeholder="stdio command" /><Input bind:value={capability_args} aria-label="Capability arguments" placeholder="stdio arguments" /></div><Input bind:value={capability_url} aria-label="Capability HTTP URL" placeholder="Streamable HTTP URL" /><div class="flex flex-wrap gap-2"><Button size="sm" variant="outline" onclick={yield* PreviewCapability()}><Cloud size={14} />Preview connection</Button>{#if capability_preview}<Badge variant="secondary">{capability_preview.candidate_name} · {capability_preview.discovery_status}</Badge><Button size="sm" onclick={yield* RequestCapability()}>Request approval</Button><Button size="sm" variant="outline" onclick={yield* After(api?.Actions.DecideCapabilityConnect({ approval_id: capability_approval_id, preview_fingerprint: capability_preview.preview_fingerprint, approved: true }) ?? Effect.void)}>Approve</Button><Button size="sm" variant="ghost" onclick={yield* After(api?.Actions.DecideCapabilityConnect({ approval_id: capability_approval_id, preview_fingerprint: capability_preview.preview_fingerprint, approved: false }) ?? Effect.void)}>Deny</Button>{/if}</div>{#if capability_preview}<p class="text-muted-foreground text-xs">Tools: {capability_preview.tools.map((tool) => tool.name).join(", ") || "discovered after connection"}. Permissions: {capability_preview.permissions.map((item) => item.kind).join(", ") || "none"}.</p>{/if}</CardContent></Card>
		</div></ScrollArea>
		{#if RoutineDetail}<Card><CardHeader><CardTitle class="flex flex-wrap items-center gap-2 text-sm">{RoutineDetail.display_name}<Badge variant="outline">{RoutineDetail.status}</Badge></CardTitle></CardHeader><CardContent class="grid gap-2 text-sm"><p>{RoutineDetail.description}</p><p class="text-muted-foreground text-xs">{RoutineDetail.source.kind}: {RoutineDetail.source.locator} · {RoutineDetail.trust} · {RoutineDetail.compatibility.map((item) => `${item.engine_id}: ${item.state}`).join(", ") || "No compatibility declared"}</p><p class="text-muted-foreground text-xs">Permissions: {RoutineDetail.permissions.map((item) => item.kind).join(", ") || "none"}. Commands: {RoutineDetail.exported_commands.map((item) => item.name).join(", ") || "none"}. Sync: {RoutineDetail.sync.map((item) => `${item.engine_id}: ${item.status}`).join(", ") || "none"}.</p><div class="flex flex-wrap gap-2"><Button size="sm" variant="outline" onclick={yield* After((RoutineDetail.enabled ? api?.Actions.DisableRoutine : api?.Actions.EnableRoutine)?.({ routine_id: RoutineDetail.id, scope: RoutineDetail.scope }) ?? Effect.void)}>{RoutineDetail.enabled ? "Disable" : "Enable"}</Button><Button size="sm" variant="outline" onclick={yield* After(api?.Actions.SyncRoutine({ id: RoutineDetail.id, scope: RoutineDetail.scope, engine_id: "codex" }) ?? Effect.void)}>Sync Codex</Button><Button size="sm" variant="outline" onclick={yield* After(api?.Actions.RollbackRoutine({ routine_id: RoutineDetail.id, rollback_id: RoutineDetail.id, scope: RoutineDetail.scope }) ?? Effect.void)}><Restart size={14} />Rollback</Button><Button size="sm" variant="destructive" onclick={yield* After(api?.Actions.RemoveRoutine({ routine_id: RoutineDetail.id, scope: RoutineDetail.scope }) ?? Effect.void)}><Trash size={14} />Remove</Button></div><div class="flex gap-2"><Input bind:value={routine_task} placeholder="Task summary" /><Input bind:value={routine_command} placeholder="Optional routine command" /><Button size="sm" onclick={yield* Run(api?.Actions.InvokeRoutine({ routine_id: RoutineDetail.id, scope: RoutineDetail.scope, task_summary: routine_task, command: routine_command || undefined }) ?? Effect.void)}>Invoke</Button></div></CardContent></Card>{/if}
		{#if CapabilityDetail}<Card><CardHeader><CardTitle class="flex flex-wrap items-center gap-2 text-sm">{CapabilityDetail.display_name}<Badge variant="outline">{CapabilityDetail.lifecycle} · {CapabilityDetail.health.status}</Badge></CardTitle></CardHeader><CardContent class="grid gap-2 text-sm"><p class="text-muted-foreground text-xs">{CapabilityDetail.transport.kind} · {CapabilityDetail.trust} · {CapabilityDetail.sync.map((item) => `${item.engine_id}: ${item.status}`).join(", ") || "not synced"}</p><p class="text-muted-foreground text-xs">Tools: {CapabilityDetail.tools.map((tool) => tool.name).join(", ") || "none"}. Resources: {CapabilityDetail.resources.map((resource) => resource.uri).join(", ") || "none"}. {CapabilityDetail.server_instructions ?? ""}</p><Accordion type="single" collapsible><AccordionItem value="policy"><AccordionTrigger>Permissions and tool policy</AccordionTrigger><AccordionContent>{CapabilityDetail.permissions.map((item) => `${item.kind}: ${item.description}`).join(" · ") || "No permissions"}<br />{CapabilityDetail.policy.map((item) => `${item.name}: ${item.approval}`).join(" · ") || "No tool policy"}</AccordionContent></AccordionItem><AccordionItem value="oauth"><AccordionTrigger>Authentication ({CapabilityDetail.auth.kind})</AccordionTrigger><AccordionContent class="flex flex-wrap gap-2"><Badge variant="secondary">{OAuth?.status ?? "not checked"}</Badge><Button size="sm" variant="outline" onclick={yield* Run(api?.Actions.GetCapabilityOAuthStatus({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope }) ?? Effect.void)}>Refresh status</Button><Button size="sm" variant="outline" onclick={yield* Run(api?.Actions.BeginCapabilityOAuth({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope }) ?? Effect.void)}><Key size={14} />Begin OAuth</Button><Input bind:value={oauth_callback} placeholder="Opaque callback reference" /><Button size="sm" onclick={yield* After(api?.Actions.CompleteCapabilityOAuth({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope, callback_reference: oauth_callback }) ?? Effect.void)}>Complete</Button><Button size="sm" variant="outline" onclick={yield* After(api?.Actions.RefreshCapabilityOAuth({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope }) ?? Effect.void)}>Refresh token</Button><Button size="sm" variant="destructive" onclick={yield* After(api?.Actions.RevokeCapabilityOAuth({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope }) ?? Effect.void)}>Revoke</Button></AccordionContent></AccordionItem></Accordion><div class="flex flex-wrap gap-2"><Button size="sm" variant="outline" onclick={yield* After((CapabilityDetail.enabled ? api?.Actions.DisableCapability : api?.Actions.EnableCapability)?.({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope }) ?? Effect.void)}>{CapabilityDetail.enabled ? "Disable" : "Enable"}</Button><Button size="sm" variant="outline" onclick={yield* After(api?.Actions.StartCapability({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope }) ?? Effect.void)}>Start</Button><Button size="sm" variant="outline" onclick={yield* After(api?.Actions.ReconnectCapability({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope }) ?? Effect.void)}>Reconnect</Button><Button size="sm" variant="outline" onclick={yield* After(api?.Actions.RestartCapability({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope }) ?? Effect.void)}>Restart</Button><Button size="sm" variant="outline" onclick={yield* After(api?.Actions.CheckCapabilityHealth({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope }) ?? Effect.void)}>Health</Button><Button size="sm" variant="outline" onclick={yield* After(api?.Actions.SyncCapability({ id: CapabilityDetail.id, scope: CapabilityDetail.scope, engine_id: "codex" }) ?? Effect.void)}>Sync Codex</Button><Button size="sm" variant="destructive" onclick={yield* After(api?.Actions.UninstallCapability({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope }) ?? Effect.void)}>Uninstall</Button></div><div class="grid gap-2 sm:grid-cols-[1fr_1fr_auto]"><Input bind:value={invocation_tool} placeholder="Tool name" /><Textarea bind:value={invocation_args} rows={1} placeholder="JSON arguments" /><Button size="sm" onclick={yield* Run(api?.Actions.InvokeCapability({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope, tool_name: invocation_tool, arguments_json: invocation_args }) ?? Effect.void)}>Invoke</Button></div></CardContent></Card>{/if}
		{#if RoutineDetail || CapabilityDetail}<Card><CardHeader><CardTitle class="text-sm">Approval and drift actions</CardTitle></CardHeader><CardContent class="grid gap-2"><div class="grid gap-2 sm:grid-cols-4"><Input bind:value={drift_engine} placeholder="Engine id (Codex)" /><Input bind:value={drift_revision} placeholder="Observed revision" /><Input bind:value={drift_approval} placeholder="Approval id" /><Input bind:value={drift_fingerprint} placeholder="Intent fingerprint" /></div>{#if RoutineDetail}<div class="flex flex-wrap gap-2"><Button size="sm" variant="outline" onclick={yield* After(api?.Actions.ResolveRoutineDrift({ routine_id: RoutineDetail.id, scope: RoutineDetail.scope, engine_id: drift_engine, observed_revision: drift_revision, action: "import" }) ?? Effect.void)}>Import routine drift</Button><Button size="sm" variant="outline" onclick={yield* After(api?.Actions.ResolveRoutineDrift({ routine_id: RoutineDetail.id, scope: RoutineDetail.scope, engine_id: drift_engine, observed_revision: drift_revision, action: "ignore" }) ?? Effect.void)}>Ignore routine drift</Button><Button size="sm" variant="outline" onclick={yield* After(api?.Actions.RequestRoutineDriftOverwrite({ routine_id: RoutineDetail.id, scope: RoutineDetail.scope, engine_id: drift_engine, observed_revision: drift_revision, approval_id: drift_approval, intent_fingerprint: drift_fingerprint, requested_by: "user" }) ?? Effect.void)}>Request overwrite approval</Button><Button size="sm" onclick={yield* After(api?.Actions.DecideRoutineDriftOverwrite({ routine_id: RoutineDetail.id, scope: RoutineDetail.scope, engine_id: drift_engine, observed_revision: drift_revision, approval_id: drift_approval, intent_fingerprint: drift_fingerprint, approved: true }) ?? Effect.void)}>Approve overwrite</Button><Button size="sm" variant="ghost" onclick={yield* After(api?.Actions.DecideRoutineDriftOverwrite({ routine_id: RoutineDetail.id, scope: RoutineDetail.scope, engine_id: drift_engine, observed_revision: drift_revision, approval_id: drift_approval, intent_fingerprint: drift_fingerprint, approved: false }) ?? Effect.void)}>Deny overwrite</Button></div>{/if}{#if CapabilityDetail}<div class="flex flex-wrap gap-2"><Button size="sm" variant="outline" onclick={yield* After(api?.Actions.DisconnectCapability({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope }) ?? Effect.void)}>Disconnect</Button><Button size="sm" variant="destructive" onclick={yield* After(api?.Actions.RemoveCapability({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope }) ?? Effect.void)}>Remove record</Button><Button size="sm" variant="outline" onclick={yield* After(api?.Actions.ResolveCapabilityDrift({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope, engine_id: drift_engine, observed_revision: drift_revision, action: "import" }) ?? Effect.void)}>Import capability drift</Button><Button size="sm" variant="outline" onclick={yield* After(api?.Actions.ResolveCapabilityDrift({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope, engine_id: drift_engine, observed_revision: drift_revision, action: "ignore" }) ?? Effect.void)}>Ignore capability drift</Button><Button size="sm" variant="outline" onclick={yield* After(api?.Actions.RequestCapabilityDriftOverwrite({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope, engine_id: drift_engine, observed_revision: drift_revision, approval_id: drift_approval, intent_fingerprint: drift_fingerprint, requested_by: "user" }) ?? Effect.void)}>Request overwrite approval</Button><Button size="sm" onclick={yield* After(api?.Actions.DecideCapabilityDriftOverwrite({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope, engine_id: drift_engine, observed_revision: drift_revision, approval_id: drift_approval, intent_fingerprint: drift_fingerprint, approved: true }) ?? Effect.void)}>Approve overwrite</Button><Button size="sm" variant="ghost" onclick={yield* After(api?.Actions.DecideCapabilityDriftOverwrite({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope, engine_id: drift_engine, observed_revision: drift_revision, approval_id: drift_approval, intent_fingerprint: drift_fingerprint, approved: false }) ?? Effect.void)}>Deny overwrite</Button></div><div class="flex flex-wrap gap-2"><Input bind:value={invocation_approval} placeholder="Invocation approval id" /><Input bind:value={invocation_fingerprint} placeholder="Invocation intent fingerprint" /><Button size="sm" variant="outline" onclick={yield* Run(api?.Actions.RequestCapabilityInvocation({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope, tool_name: invocation_tool, arguments_json: invocation_args, approval_id: invocation_approval, intent_fingerprint: invocation_fingerprint, requested_by: "user" }) ?? Effect.void)}>Request tool approval</Button><Button size="sm" onclick={yield* Run(api?.Actions.DecideCapabilityInvocation({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope, tool_name: invocation_tool, arguments_json: invocation_args, approval_id: invocation_approval, intent_fingerprint: invocation_fingerprint, approved: true }) ?? Effect.void)}>Approve tool</Button><Button size="sm" variant="ghost" onclick={yield* Run(api?.Actions.DecideCapabilityInvocation({ capability_id: CapabilityDetail.id, scope: CapabilityDetail.scope, tool_name: invocation_tool, arguments_json: invocation_args, approval_id: invocation_approval, intent_fingerprint: invocation_fingerprint, approved: false }) ?? Effect.void)}>Deny tool</Button></div>{/if}</CardContent></Card>{/if}
		{#if CapabilityDetail}
			<Card>
				<CardHeader><CardTitle class="text-sm">OAuth authorization handoff</CardTitle></CardHeader>
				<CardContent class="grid gap-2 text-xs">
					{#if oauth_authorization_url}
						<code class="break-all rounded-md bg-muted p-2">{oauth_authorization_url}</code>
						<div class="flex flex-wrap gap-2"><Button size="xs" variant="outline" onclick={yield* CopyOAuthUrl()}>Copy authorization URL</Button><Badge variant="secondary">Continuation {oauth_continuation_reference}</Badge></div>
					{/if}
					<p class="text-muted-foreground">Open the authorization URL in your external browser, then paste the opaque callback reference returned by the connector.</p>
				</CardContent>
			</Card>
		{/if}
	</DialogContent>
</Dialog>
