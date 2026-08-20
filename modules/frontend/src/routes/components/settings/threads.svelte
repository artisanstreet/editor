<script lang="ts" effect>
	import { Effect, Stream } from "effect";
	import type { ThreadRetentionPolicy } from "@artisan/protocol";
	import { ThreadRetentionPolicyController } from "$lib/settings/thread-retention-policy-controller";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Switch } from "$lib/components/ui/switch";
	import AgentNames from "./agent-names.svelte";
	import Card from "./card.svelte";
	import Header from "./header.svelte";
	import Row from "./row.svelte";
	import Section from "./section.svelte";
	import UsageRecovery from "./usage-recovery.svelte";

	const retention_controller = yield* ThreadRetentionPolicyController;

	const initial_policy_state = yield* retention_controller.Current;
	let policy_state = $state.raw(initial_policy_state);
	const policy = $derived(
		policy_state._tag === "Ready" ? policy_state.policy : undefined,
	);
	let days_text = $state(
		initial_policy_state._tag === "Ready"
			? String(initial_policy_state.policy.inactivity_days)
			: "",
	);
	let policy_failure_title = $state("Could not load retention policy");
	let saving = $state(false);
	const ApplyPolicyState = (next: typeof policy_state) =>
		Effect.sync(() => {
			policy_state = next;
			if (next._tag === "Ready") {
				policy_failure_title = "";
				days_text = String(next.policy.inactivity_days);
			}
		});
	yield* retention_controller.Changes.pipe(Stream.runForEach(ApplyPolicyState), Effect.forkScoped);

	const RetryPolicyLoad = Effect.gen(function* () {
		policy_state = { _tag: "Loading" };
		days_text = "";
		yield* retention_controller.Refresh.pipe(
			Effect.catch(() =>
				Effect.sync(() => {
					policy_failure_title = "Could not load retention policy";
				}),
			),
		);
	});

	const SavePolicy = (next: ThreadRetentionPolicy) =>
		Effect.gen(function* () {
			saving = true;
			yield* retention_controller.Save(next);
		}).pipe(
			Effect.catch(() =>
				Effect.gen(function* () {
				policy_failure_title = "Could not save retention policy";
				days_text = "";
				}),
			),
			Effect.ensuring(
				Effect.gen(function* () {
					saving = false;
				}),
			),
		);

	const ToggleEnabled = (enabled: boolean) =>
		Effect.gen(function* () {
			if (policy === undefined || saving) return;
			yield* SavePolicy({ ...policy, enabled });
		});

	/** Commits on blur/enter; an out-of-range or unparsable value reverts. */
	const CommitDays = Effect.gen(function* () {
		if (policy === undefined || saving) return;
		const parsed = Number(days_text);
		if (!Number.isSafeInteger(parsed) || parsed < 1 || parsed > 3650) {
			days_text = String(policy.inactivity_days);
			return;
		}
		if (parsed !== policy.inactivity_days) {
			yield* SavePolicy({ ...policy, inactivity_days: parsed });
		}
	});

	const CommitDaysOnEnter = (event: KeyboardEvent) =>
		Effect.gen(function* () {
			if (event.key !== "Enter") return;
			yield* CommitDays;
		});

	/**
	 * The hoisted program rather than an inline recovery: naming `policy_state` at
	 * a top-level yield site makes it one of the site's reactive inputs, and the
	 * load writes that same state. Inline, the site would re-run on its own answer
	 * and re-query the Forge for as long as this screen stayed open.
	 */
	if (initial_policy_state._tag !== "Ready") yield* RetryPolicyLoad.pipe(Effect.forkScoped);
</script>

<Header title="Threads" description="Lifecycle rules the Forge applies to every thread." />

<Section id="retention" title="Retention">
	{#if policy_state._tag === "Unverified"}
		<div
			class="mt-3 flex flex-col items-start justify-between gap-3 rounded-xl border border-warning/30 bg-warning/8 px-3 py-3 text-sm text-foreground sm:flex-row sm:items-center sm:gap-4"
			role="status"
		>
			<span class="flex flex-col gap-0.5">
				<span class="font-medium">{policy_failure_title}</span>
				<span class="text-xs leading-relaxed text-muted-foreground">
					Forge could not confirm the durable policy. Controls are disabled.
				</span>
			</span>
			<Button
				variant="outline"
				size="sm"
				disabled={saving}
				onclick={yield* RetryPolicyLoad}
			>
				Retry
			</Button>
		</div>
	{/if}
	{#if policy_state._tag === "Loading"}
		<Card class="mt-3">
			<div class="flex flex-col gap-3 px-4 py-4" aria-label="Loading retention policy" role="status">
				<div class="h-4 w-2/5 animate-pulse rounded bg-muted"></div>
				<div class="h-8 w-full animate-pulse rounded bg-muted/70"></div>
			</div>
		</Card>
	{:else}
	<Card class="mt-3">
		<Row
			title="Erase inactive threads"
			description="Permanently erases a thread — conversation, checkpoints, and lineage — once it has been untouched for the configured number of days. This is deletion, not archival."
		>
			{#snippet control()}
				<Switch
					checked={policy?.enabled ?? false}
					disabled={policy === undefined || saving}
					aria-label="Erase inactive threads"
					onclick={yield* ToggleEnabled(!(policy?.enabled ?? false))}
				/>
			{/snippet}
		</Row>
		<Row
			title="Inactivity threshold"
			description="Days a thread must be untouched before it is erased. Between 1 and 3650."
		>
			{#snippet control()}
				<Input
					type="number"
					min={1}
					max={3650}
					class="h-8 w-24 text-right text-sm"
					disabled={policy === undefined || policy.enabled === false || saving}
					aria-label="Inactivity threshold in days"
					bind:value={days_text}
					onblur={yield* CommitDays}
					onkeydown={yield* CommitDaysOnEnter(event)}
				/>
			{/snippet}
		</Row>
	</Card>
	{/if}
</Section>

<UsageRecovery />
<AgentNames />
