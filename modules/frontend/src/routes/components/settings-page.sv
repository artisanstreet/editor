<script lang="ts" effect>
	import { Effect, Option, Stream } from "effect";
	import type { ThreadRetentionPolicy, ThreadSessionPolicy } from "@artisan/protocol";

	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "$lib/components/ui/card";
	import {
		Dialog,
		DialogContent,
		DialogDescription,
		DialogFooter,
		DialogHeader,
		DialogTitle,
	} from "$lib/components/ui/dialog";
	import { Input } from "$lib/components/ui/input";
	import {
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
	} from "$lib/components/ui/select";
	import { Switch } from "$lib/components/ui/switch";
	import { Textarea } from "$lib/components/ui/textarea";
	import { LiveWorkspaceStore, type LiveWorkspaceSnapshot } from "$lib/live-workspace/store";

	const live_workspace = yield* LiveWorkspaceStore;
	let live_snapshot = $state.raw<LiveWorkspaceSnapshot>(yield* live_workspace.Snapshot);
	let local_error = $state<string | undefined>();
	let guidance_open = $state(false);
	let guidance_draft = $state("");
	let policy_fingerprint = $state("");
	let policy_model = $state("");
	let policy_reasoning = $state<ThreadSessionPolicy["reasoning_effort"]>("medium");
	let policy_permission = $state<ThreadSessionPolicy["permission_mode"]>("on_request");
	let policy_sandbox = $state<ThreadSessionPolicy["sandbox_mode"]>("workspace_write");
	let policy_web_search = $state(false);
	let policy_strict_clarification = $state(false);
	let retention_policy = $state<ThreadRetentionPolicy>();

	yield* Stream.runForEach(live_workspace.Changes, (next_snapshot) =>
		Effect.sync(() => {
			live_snapshot = next_snapshot;
		}),
	).pipe(Effect.forkScoped);

	const selected_thread_id = $derived(Option.getOrUndefined(live_snapshot.selected_thread_id));
	const session = $derived(Option.getOrUndefined(live_snapshot.session));
	const guidance = $derived(Option.getOrUndefined(live_snapshot.global_guidance));
	const model_behaviour = $derived(Option.getOrUndefined(live_snapshot.model_behaviour));

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

	const OpenGuidance = Effect.sync(() => {
		guidance_draft = guidance?.content ?? "";
		guidance_open = true;
	});

	const SaveGuidance = Effect.gen(function* () {
		yield* Run(live_workspace.Actions.UpdateGlobalGuidance({ content: guidance_draft }), live_workspace.Refresh);
		if (local_error === undefined) guidance_open = false;
	});

	const SaveSessionPolicy = Effect.gen(function* () {
		if (selected_thread_id === undefined) return;
		const model = policy_model.trim();
		yield* Run(
			live_workspace.Actions.UpdateThreadSessionPolicy({
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
			live_workspace.Refresh,
		);
	});

	const LoadRetentionPolicy = () =>
		live_workspace.Actions.GetThreadRetentionPolicy.pipe(
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
			live_workspace.Actions.UpdateThreadRetentionPolicy(retention_policy),
			LoadRetentionPolicy(),
		);
	});

	type GuidanceDriftInput = Parameters<typeof live_workspace.Actions.ResolveGlobalGuidanceDrift>[0];
	type ModelDriftInput = Parameters<typeof live_workspace.Actions.ResolveModelBehaviourDrift>[0];
	const SelectGuidanceCandidate = (
		provider: Parameters<typeof live_workspace.Actions.SelectGlobalGuidance>[0]["provider"],
		content_hash: string,
	) => Run(live_workspace.Actions.SelectGlobalGuidance({ provider, content_hash }), live_workspace.Refresh);
	const ResolveGuidanceDrift = (
		provider: GuidanceDriftInput["provider"],
		observed_hash: string,
		action: GuidanceDriftInput["action"],
	) => Run(live_workspace.Actions.ResolveGlobalGuidanceDrift({ provider, observed_hash, action }), live_workspace.Refresh);
	const RetryGuidanceSync = (
		provider: Parameters<typeof live_workspace.Actions.RetryGlobalGuidanceSync>[0]["provider"],
	) => Run(live_workspace.Actions.RetryGlobalGuidanceSync({ provider }), live_workspace.Refresh);
	const UpdateModelSetting = (
		setting_id: Parameters<typeof live_workspace.Actions.UpdateModelBehaviour>[0]["setting_id"],
		value: string,
	) =>
		Run(
			live_workspace.Actions.UpdateModelBehaviour({
				setting_id,
				value: value.trim().length === 0 ? { type: "provider_default" } : { type: "integer", value: Number(value) },
			}),
			live_workspace.Refresh,
		);
	const ResolveModelDrift = (
		provider_id: string,
		setting_id: ModelDriftInput["setting_id"],
		observed_hash: string,
		action: ModelDriftInput["action"],
	) => Run(live_workspace.Actions.ResolveModelBehaviourDrift({ provider_id, setting_id, observed_hash, action }), live_workspace.Refresh);
	const RetryModelSync = (
		provider_id: string,
		setting_id: Parameters<typeof live_workspace.Actions.RetryModelBehaviourSync>[0]["setting_id"],
	) => Run(live_workspace.Actions.RetryModelBehaviourSync({ provider_id, setting_id }), live_workspace.Refresh);
</script>

<main class="settings-page" aria-label="Settings">
	<header class="settings-intro">
		<div>
			<p class="eyebrow">Artisan Editor</p>
			<h1>Settings</h1>
			<p>Preferences and backend-backed controls in one scrollable place.</p>
		</div>
		<Badge variant={live_snapshot.phase === "ready" ? "default" : "secondary"}>{live_snapshot.phase}</Badge>
	</header>

	<section id="general" class="settings-section" aria-labelledby="general-title">
		<Card>
			<CardHeader><CardTitle id="general-title">General</CardTitle><CardDescription>Connection and session context are always read from the desktop backend.</CardDescription></CardHeader>
			<CardContent class="settings-grid">
				<div class="settings-row"><span>Engine</span><Badge variant="secondary">Codex CLI</Badge></div>
				<div class="settings-row"><span>Selected thread</span><code>{selected_thread_id ?? "No thread selected"}</code></div>
				<div class="settings-row"><span>Thread state</span><Badge variant="outline">{session?.latest_intake?.resolution ?? "No active session"}</Badge></div>
			</CardContent>
		</Card>
	</section>

	<section id="codex" class="settings-section" aria-labelledby="codex-title">
		<Card>
			<CardHeader><CardTitle id="codex-title">Codex</CardTitle><CardDescription>Policy for the currently selected thread. Choose a thread to make changes.</CardDescription></CardHeader>
			<CardContent class="settings-grid">
				<label class="settings-field"><span>Model override</span><Input bind:value={policy_model} placeholder="Codex default" disabled={session === undefined} /></label>
				<label class="settings-field"><span>Reasoning effort</span><Select bind:value={policy_reasoning} disabled={session === undefined}><SelectTrigger class="w-full">{policy_reasoning}</SelectTrigger><SelectContent><SelectItem value="low">Low</SelectItem><SelectItem value="medium">Medium</SelectItem><SelectItem value="high">High</SelectItem><SelectItem value="xhigh">Extra high</SelectItem></SelectContent></Select></label>
				<label class="settings-field"><span>Permission mode</span><Select bind:value={policy_permission} disabled={session === undefined}><SelectTrigger class="w-full">{policy_permission === "on_request" ? "Ask when needed" : "Never ask"}</SelectTrigger><SelectContent><SelectItem value="on_request">Ask when needed</SelectItem><SelectItem value="never">Never ask</SelectItem></SelectContent></Select></label>
				<label class="settings-field"><span>Sandbox</span><Select bind:value={policy_sandbox} disabled={session === undefined}><SelectTrigger class="w-full">{policy_sandbox === "workspace_write" ? "Workspace write" : "Read only"}</SelectTrigger><SelectContent><SelectItem value="workspace_write">Workspace write</SelectItem><SelectItem value="read_only">Read only</SelectItem></SelectContent></Select></label>
				<div class="settings-row"><label for="web-search">Web search</label><Switch id="web-search" checked={policy_web_search} disabled={session === undefined} onclick={() => (policy_web_search = !policy_web_search)} /></div>
				<div class="settings-row"><label for="strict-clarification">Strict clarification</label><Switch id="strict-clarification" checked={policy_strict_clarification} disabled={session === undefined} onclick={() => (policy_strict_clarification = !policy_strict_clarification)} /></div>
				<Button class="justify-self-start" disabled={session === undefined} onclick={yield* SaveSessionPolicy}>Save thread policy</Button>
			</CardContent>
		</Card>
	</section>

	<section id="guidance" class="settings-section" aria-labelledby="guidance-title">
		<Card>
			<CardHeader><CardTitle id="guidance-title">Guidance</CardTitle><CardDescription>Canonical instructions are synchronized through the backend.</CardDescription></CardHeader>
			<CardContent class="settings-grid">
				<p class="whitespace-pre-wrap text-sm text-muted-foreground">{guidance?.content ?? "Global guidance is unavailable."}</p>
				<Button class="justify-self-start" variant="outline" disabled={guidance === undefined} onclick={yield* OpenGuidance}>Edit guidance</Button>
				{#each guidance?.candidates ?? [] as candidate (candidate.content_hash)}<div class="settings-item"><div class="flex items-center gap-2"><Badge variant="outline">First-run candidate</Badge><span>{candidate.provider}</span></div><p>{candidate.preview}</p><Button class="w-fit" size="xs" onclick={yield* SelectGuidanceCandidate(candidate.provider, candidate.content_hash)}>Use this guidance</Button></div>{/each}
				{#each guidance?.metadata.providers ?? [] as provider (provider.provider)}<div class="settings-item"><div class="flex items-center gap-2"><span>{provider.provider}</span><Badge variant="secondary">{provider.status}</Badge></div>{#if provider.last_error_code}<code class="text-destructive">{provider.last_error_code}</code>{/if}<div class="flex flex-wrap gap-1">{#if provider.status === "drift_detected" && provider.observed_hash}<Button size="xs" variant="outline" onclick={yield* ResolveGuidanceDrift(provider.provider, provider.observed_hash, "import")}>Import</Button><Button size="xs" variant="outline" onclick={yield* ResolveGuidanceDrift(provider.provider, provider.observed_hash, "overwrite")}>Overwrite</Button><Button size="xs" variant="ghost" onclick={yield* ResolveGuidanceDrift(provider.provider, provider.observed_hash, "ignore")}>Ignore</Button>{/if}{#if provider.status === "sync_failed"}<Button size="xs" variant="outline" onclick={yield* RetryGuidanceSync(provider.provider)}>Retry sync</Button>{/if}</div></div>{/each}
			</CardContent>
		</Card>
	</section>

	<section id="model-behaviour" class="settings-section" aria-labelledby="model-behaviour-title">
		<Card>
			<CardHeader><CardTitle id="model-behaviour-title">Model behaviour</CardTitle><CardDescription>Provider capabilities and synchronized model controls.</CardDescription></CardHeader>
			<CardContent class="settings-grid">
				{#each model_behaviour?.settings ?? [] as setting (setting.setting_id)}{@const capability = model_behaviour?.capabilities.find((item) => item.setting_id === setting.setting_id)}<div class="settings-item"><label class="settings-field"><span>{capability?.display_name ?? setting.setting_id}</span><Input type="number" min={capability?.control.minimum} max={capability?.control.maximum} step={capability?.control.step} value={setting.value.type === "integer" ? setting.value.value : ""} placeholder="Provider default" aria-label={setting.setting_id} onchange={yield* UpdateModelSetting(setting.setting_id, event.currentTarget.value)} /></label><p>{capability?.description}</p>{#each capability?.provider_support ?? [] as support (support.provider_id)}<div class="settings-row"><span>{support.provider_id}</span><Badge variant="outline">{support.state}</Badge></div>{/each}</div>{:else}<p class="text-sm text-muted-foreground">No model behaviour controls are available.</p>{/each}
				{#each model_behaviour?.providers ?? [] as provider (`${provider.provider_id}:${provider.setting_id}`)}<div class="settings-item"><div class="flex items-center gap-2"><span>{provider.provider_id}</span><Badge variant="secondary">{provider.status}</Badge></div>{#if provider.last_error_code}<code class="text-destructive">{provider.last_error_code}</code>{/if}<div class="flex flex-wrap gap-1">{#if provider.status === "drift_detected" && provider.observed_hash}<Button size="xs" variant="outline" onclick={yield* ResolveModelDrift(provider.provider_id, provider.setting_id, provider.observed_hash, "import")}>Import</Button><Button size="xs" variant="outline" onclick={yield* ResolveModelDrift(provider.provider_id, provider.setting_id, provider.observed_hash, "overwrite")}>Overwrite</Button><Button size="xs" variant="ghost" onclick={yield* ResolveModelDrift(provider.provider_id, provider.setting_id, provider.observed_hash, "ignore")}>Ignore</Button>{/if}{#if provider.status === "sync_failed"}<Button size="xs" variant="outline" onclick={yield* RetryModelSync(provider.provider_id, provider.setting_id)}>Retry sync</Button>{/if}</div></div>{/each}
			</CardContent>
		</Card>
	</section>

	<section id="retention" class="settings-section" aria-labelledby="retention-title">
		<Card>
			<CardHeader><CardTitle id="retention-title">Retention</CardTitle><CardDescription>Controls the backend’s inactive-thread cleanup policy.</CardDescription></CardHeader>
			<CardContent class="settings-grid">
				{#if retention_policy === undefined}<p class="text-sm text-muted-foreground">Load the global inactive-thread cleanup policy before editing it.</p><Button class="justify-self-start" variant="outline" onclick={yield* LoadRetentionPolicy()}>Load retention policy</Button>{:else}<div class="settings-row"><label for="retention-enabled">Auto-delete inactive threads</label><Switch id="retention-enabled" checked={retention_policy.enabled} onclick={() => retention_policy !== undefined && (retention_policy = { ...retention_policy, enabled: !retention_policy.enabled })} /></div><label class="settings-field"><span>Inactive days</span><Input type="number" min="1" max="3650" value={retention_policy.inactivity_days} onchange={(event) => retention_policy !== undefined && (retention_policy = { ...retention_policy, inactivity_days: Number(event.currentTarget.value) })} /></label><Button class="justify-self-start" onclick={yield* SaveRetentionPolicy}>Save retention</Button>{/if}
			</CardContent>
		</Card>
	</section>

	<section id="appearance" class="settings-section" aria-labelledby="appearance-title">
		<Card>
			<CardHeader><CardTitle id="appearance-title">Appearance</CardTitle><CardDescription>The desktop follows the system-aware theme selected at launch.</CardDescription></CardHeader>
			<CardContent class="settings-grid"><div class="settings-row"><span>Interface density</span><Badge variant="outline">Comfortable</Badge></div><p class="text-sm text-muted-foreground">Theme controls will live here once they are persisted by the desktop shell; this page does not expose a non-persistent toggle.</p></CardContent>
		</Card>
	</section>

	{#if local_error !== undefined}<p class="settings-error" role="alert">{local_error}</p>{/if}
	{#if Option.isSome(live_snapshot.error)}<p class="settings-error" role="alert">{live_snapshot.error.value}</p>{/if}
</main>

<Dialog bind:open={guidance_open}>
	<DialogContent>
		<DialogHeader><DialogTitle>Edit global guidance</DialogTitle><DialogDescription>This canonical guidance is synchronized through the backend.</DialogDescription></DialogHeader>
		<Textarea bind:value={guidance_draft} class="min-h-64 font-mono" aria-label="Global guidance" />
		<DialogFooter><Button variant="outline" onclick={() => (guidance_open = false)}>Cancel</Button><Button onclick={yield* SaveGuidance}>Save</Button></DialogFooter>
	</DialogContent>
</Dialog>

<style>
	.settings-page { max-width: 60rem; margin: 0 auto; padding: 3rem clamp(1rem, 4vw, 3rem) 6rem; }
	.settings-intro { display: flex; align-items: start; justify-content: space-between; gap: 1rem; margin-bottom: 2rem; }
	.settings-intro h1 { margin: 0.25rem 0; font-size: 2rem; font-weight: 600; letter-spacing: -0.04em; }
	.settings-intro p:not(.eyebrow) { margin: 0; color: var(--muted-foreground); }
	.eyebrow { margin: 0; color: var(--muted-foreground); font-size: 0.75rem; font-weight: 600; letter-spacing: 0.08em; text-transform: uppercase; }
	.settings-section { scroll-margin-top: 1rem; margin-top: 1rem; }
	:global(.settings-grid) { display: grid; gap: 1rem; }
	.settings-row { display: flex; min-height: 2rem; align-items: center; justify-content: space-between; gap: 1rem; font-size: 0.875rem; }
	.settings-field { display: grid; gap: 0.4rem; font-size: 0.875rem; font-weight: 500; }
	.settings-item { display: grid; gap: 0.5rem; border: 1px solid var(--border); border-radius: var(--radius); padding: 0.875rem; font-size: 0.875rem; }
	.settings-item p { margin: 0; color: var(--muted-foreground); }
	.settings-error { margin-top: 1rem; color: var(--destructive); font-size: 0.875rem; }
	@media (max-width: 640px) { .settings-page { padding-top: 1.5rem; } }
</style>
