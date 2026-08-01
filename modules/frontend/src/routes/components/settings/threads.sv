<script lang="ts" effect>
	import { Effect } from "effect";
	import type { ThreadRetentionPolicy } from "@artisan/protocol";
	import { ArtisanClient } from "@artisan/transport/client";
	import { BannerService } from "$lib/banner/service";
	import { Input } from "$lib/components/ui/input";
	import { Switch } from "$lib/components/ui/switch";
	import Row from "./row.sv";

	const client = yield* ArtisanClient;
	const banner = yield* BannerService;

	type RetentionPolicyState =
		| { readonly _tag: "Loading" }
		| { readonly _tag: "Ready"; readonly policy: ThreadRetentionPolicy }
		| { readonly _tag: "Unverified" };

	let policy_state = $state.raw<RetentionPolicyState>({ _tag: "Loading" });
	const policy = $derived(
		policy_state._tag === "Ready" ? policy_state.policy : undefined,
	);
	let days_text = $state("");
	let saving = $state(false);

	const LoadPolicy = Effect.gen(function* () {
		const current = yield* client.GetThreadRetentionPolicy;
		policy_state = { _tag: "Ready", policy: current };
		days_text = String(current.inactivity_days);
	});

	const RetryPolicyLoad = Effect.gen(function* () {
		policy_state = { _tag: "Loading" };
		days_text = "";
		yield* LoadPolicy.pipe(
			Effect.catch((error) =>
				Effect.gen(function* () {
					policy_state = { _tag: "Unverified" };
					yield* banner.error("Could not verify retention policy", {
						description: error.message,
					});
				}),
			),
		);
	});

	const SavePolicy = (next: ThreadRetentionPolicy) =>
		Effect.gen(function* () {
			saving = true;
			yield* client.UpdateThreadRetentionPolicy(next);
			policy_state = { _tag: "Ready", policy: next };
			days_text = String(next.inactivity_days);
		}).pipe(
			Effect.catch((error) =>
				Effect.gen(function* () {
					policy_state = { _tag: "Unverified" };
					days_text = "";
					yield* banner.error("Could not save retention policy", {
						description: `${error.message} Artisan could not verify whether Forge committed the change.`,
					});
					yield* LoadPolicy.pipe(
						Effect.catch((reconciliation_error) =>
							Effect.gen(function* () {
								yield* banner.error("Retention policy remains unverified", {
									description: reconciliation_error.message,
								});
							}),
						),
					);
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

	yield* LoadPolicy.pipe(
		Effect.catch((error) =>
			Effect.gen(function* () {
				policy_state = { _tag: "Unverified" };
				yield* banner.error("Could not load retention policy", { description: error.message });
			}),
		),
	);
</script>

<h1 class="text-lg font-semibold text-foreground">Threads</h1>
<p class="mt-1 text-sm text-muted-foreground">
	Lifecycle rules the Forge applies to every thread.
</p>

<section class="mt-10" aria-labelledby="retention">
	<h2 id="retention" class="scroll-mt-6 text-sm font-medium text-foreground">Retention</h2>
	{#if policy_state._tag === "Unverified"}
		<div
			class="mt-3 flex items-center justify-between gap-4 rounded-xl border border-warning/30 bg-warning/8 px-3 py-2 text-sm text-foreground"
			role="status"
		>
			<span>The durable retention policy could not be verified. Controls are disabled.</span>
			<button
				type="button"
				class="shrink-0 rounded-lg px-2 py-1 font-medium text-foreground outline-none hover:bg-foreground/5 focus-visible:ring-2 focus-visible:ring-ring/50"
				disabled={saving}
				onclick={yield* RetryPolicyLoad}
			>
				Retry
			</button>
		</div>
	{/if}
	<div
		class="card mt-3 rounded-xl bg-linear-to-b from-surface-225 to-surface-200 dark:from-surface-800 dark:to-surface-925"
	>
		<div class="flex flex-col divide-y divide-border/40">
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
		</div>
	</div>
</section>
