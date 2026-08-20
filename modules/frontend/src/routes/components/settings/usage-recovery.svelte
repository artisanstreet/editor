<script lang="ts" effect>
	import { Effect, Stream } from "effect";
	import { Switch } from "$lib/components/ui/switch";
	import {
		SessionDefaultsController,
		type SessionDefaultsState,
	} from "$lib/settings/session-defaults-controller";
	import Card from "./card.svelte";
	import Row from "./row.svelte";
	import Section from "./section.svelte";

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

<Section id="usage-recovery" title="Usage recovery">
	<Card class="mt-3">
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
			<p class="px-4 py-3 text-sm text-destructive" role="status">{message}</p>
		{/if}
	</Card>
</Section>
