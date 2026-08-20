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

	const summarized = $derived(defaults_state.defaults.thread_title_mode === "summary");

	const ToggleSummaries = (enabled: boolean) =>
		Effect.gen(function* () {
			if (saving) return;
			message = "";
			saving = true;
			yield* defaults_controller
				.SetThreadTitleMode(enabled ? "summary" : "latest_message")
				.pipe(
					Effect.catch(() =>
						Effect.gen(function* () {
							message =
								"Couldn't verify the new default. Forge did not confirm the change.";
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

<Section id="thread-titles" title="Titles">
	<Card class="mt-3">
		<Row
			title="Summary titles"
			description="Name threads with the harness's own generated summary. When off, a thread is named by the latest message you sent. A rename of your own always wins, and threads without a summary keep the message title."
		>
			{#snippet control()}
				<Switch
					checked={summarized}
					disabled={!defaults_state.available || saving}
					aria-label="Summary titles"
					onclick={yield* ToggleSummaries(!summarized)}
				/>
			{/snippet}
		</Row>
		{#if message.length > 0}
			<p class="px-4 py-3 text-sm text-destructive" role="status">{message}</p>
		{/if}
	</Card>
</Section>
