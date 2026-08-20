<script lang="ts" effect>
	import { Effect, Schema, Stream } from "effect";
	import { AgentNameDataset, AgentNameDatasets } from "@artisan/protocol";
	import { Select, SelectContent, SelectItem, SelectTrigger } from "$lib/components/ui/select";
	import {
		SessionDefaultsController,
		type SessionDefaultsState,
	} from "$lib/settings/session-defaults-controller";
	import Card from "./card.svelte";
	import Row from "./row.svelte";
	import Section from "./section.svelte";

	const defaults_controller = yield* SessionDefaultsController;
	let state = $state.raw<SessionDefaultsState>(yield* defaults_controller.Current);
	let saving = $state(false);
	const selected_dataset = $derived(
		AgentNameDatasets.find((dataset) => dataset.id === state.defaults.agent_name_dataset) ??
			AgentNameDatasets[0],
	);
	const Apply = (next: SessionDefaultsState) =>
		Effect.gen(function* () {
			state = next;
		});
	yield* defaults_controller.Changes.pipe(Stream.runForEach(Apply), Effect.forkScoped);

	const SelectDataset = (next: string) =>
		Effect.gen(function* () {
			if (saving || next === state.defaults.agent_name_dataset) return;
			const agent_name_dataset = yield* Schema.decodeUnknownEffect(AgentNameDataset)(next);
			saving = true;
			yield* defaults_controller.SetAgentNameDataset(agent_name_dataset).pipe(
				Effect.catch(() => Effect.void),
				Effect.ensuring(
					Effect.gen(function* () {
						saving = false;
					}),
				),
			);
		});
</script>

<Section
	id="agents"
	title="Agents"
	intro="Names apply only to new agents; existing identities keep their name."
>
	<Card class="mt-3">
		<Row title="Name set" description="The catalog Artisan uses when it names a new delegated agent.">
			{#snippet control()}
				<Select
					type="single"
					value={state.defaults.agent_name_dataset ?? "norwegian"}
					disabled={!state.available || saving}
					onValueChange={yield* SelectDataset(event)}
				>
					<SelectTrigger class="w-56 max-w-full" aria-label="Agent name set">
						<span class="truncate">{selected_dataset?.label ?? "Norwegian"}</span>
					</SelectTrigger>
					<SelectContent>
						{#each AgentNameDatasets as dataset (dataset.id)}
							<SelectItem value={dataset.id}>
								{dataset.label} — {dataset.description}
							</SelectItem>
						{/each}
					</SelectContent>
				</Select>
			{/snippet}
		</Row>
	</Card>
</Section>
