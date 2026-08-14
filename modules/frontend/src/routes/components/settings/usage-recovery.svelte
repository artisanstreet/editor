<script lang="ts" effect>
	import { Effect, Stream } from "effect";
	import { Switch } from "$lib/components/ui/switch";
	import {
		SessionDefaultsController,
		type SessionDefaultsState,
	} from "$lib/settings/session-defaults-controller";
	import Row from "./row.svelte";

	const defaults_controller = yield* SessionDefaultsController;
	let defaults_state = $state.raw<SessionDefaultsState>(yield* defaults_controller.Current);
	let saving = $state(false);
	let message = $state("");

	const ApplyDefaults = (next: SessionDefaultsState) =>
		Effect.gen(function* () {
			defaults_state = next;
		});
	yield* defaults_controller.Changes.pipe(Stream.runForEach(ApplyDefaults), Effect.forkScoped);

	const ToggleAutoContinue = (enabled: boolean) =>
		Effect.gen(function* () {
			if (saving) return;
			message = "";
			saving = true;
			yield* defaults_controller.SetAutoContinueUsageLimits(enabled).pipe(
				Effect.catch(() =>
					Effect.gen(function* () {
						message = "Couldn't verify the new default. Forge did not confirm the change.";
						yield* defaults_controller.Refresh.pipe(Effect.ignore);
					}),
				),
				Effect.ensuring(
					Effect.gen(function* () {
						saving = false;
					}),
				),
			);
		});
</script>

<section class="mt-10" aria-labelledby="usage-recovery">
	<h2 id="usage-recovery" class="scroll-mt-6 text-sm font-medium text-foreground">
		Usage recovery
	</h2>
	<div
		class="card mt-3 rounded-xl bg-linear-to-b from-surface-225 to-surface-200 dark:from-surface-800 dark:to-surface-925"
	>
		<Row
			title="Automatically continue after usage resets"
			description="New turns interrupted by a provider limit continue once Forge verifies the usage window has reset. You can still change this on each interruption card."
		>
			{#snippet control()}
				<Switch
					checked={defaults_state.defaults.auto_continue_usage_limits}
					disabled={!defaults_state.available || saving}
					aria-label="Automatically continue after usage resets"
					onclick={yield* ToggleAutoContinue(
						!defaults_state.defaults.auto_continue_usage_limits,
					)}
				/>
			{/snippet}
		</Row>
		{#if message.length > 0}
			<p class="px-3 pb-3 text-sm text-destructive" role="status">{message}</p>
		{/if}
	</div>
</section>
