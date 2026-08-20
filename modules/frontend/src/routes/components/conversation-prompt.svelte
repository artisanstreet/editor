<script lang="ts" effect>
	import type { ConversationItem } from "@artisan/protocol";
	import { Effect } from "effect";
	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Card, CardContent } from "$lib/components/ui/card";
	import { Input } from "$lib/components/ui/input";
	import { RunBrowserDom } from "$lib/browser/dom";

	let {
		item,
		onquestion,
	}: {
		item: Extract<ConversationItem, { type: "question" }>;
		onquestion?: (
			question_id: string,
			answers: ReadonlyArray<string>,
		) => Effect.Effect<void, { readonly message: string }>;
	} = $props();

	let answer = $state("");
	let selected = $state.raw<ReadonlyArray<string>>([]);
	const label = $derived(item.state === "requested" ? "Question" : "Answered");
	const options = $derived(item.options ?? []);
	/** A provider that enumerated its answers is asking for a choice, not prose. */
	const choosing = $derived(options.length > 0);
	const multi_select = $derived(item.multi_select === true);

	const Submit = (answers: ReadonlyArray<string>) =>
		Effect.gen(function* () {
			if (answers.length === 0 || onquestion === undefined) return;
			yield* onquestion(item.interaction_id, answers);
			answer = "";
			selected = [];
		});

	const SubmitForm = (event: SubmitEvent) =>
		Effect.gen(function* () {
			yield* RunBrowserDom(() => event.preventDefault());
			yield* Submit([answer.trim()].filter((value) => value.length > 0));
		});

	/**
	 * A single-select choice is the answer the moment it is clicked; only a
	 * multi-select question needs a separate confirmation, because until then
	 * there is no way to say the selection is finished.
	 */
	const ChooseOption = (option: string) =>
		Effect.gen(function* () {
			if (!multi_select) return yield* Submit([option]);
			selected = selected.includes(option)
				? selected.filter((value) => value !== option)
				: [...selected, option];
		});
</script>

<Card size="sm" class="max-w-(--prose-body-width) py-3">
	<CardContent class="space-y-3">
		<div class="flex items-center gap-2">
			<Badge variant="outline">{label}</Badge>
			{#if item.header !== undefined}
				<span class="text-sm text-muted-foreground">{item.header}</span>
			{/if}
		</div>
		<p class="whitespace-pre-wrap text-base leading-7">{item.prompt}</p>

		{#if item.state === "requested" && choosing}
			<div class="flex flex-col gap-2">
				{#each options as option (option.label)}
					<Button
						type="button"
						variant={selected.includes(option.label) ? "default" : "outline"}
						size="sm"
						class="h-auto flex-col items-start gap-0.5 py-2 text-left whitespace-normal"
						aria-pressed={multi_select ? selected.includes(option.label) : undefined}
						onclick={yield* ChooseOption(option.label)}
					>
						<span class="font-medium">{option.label}</span>
						{#if option.description !== undefined}
							<span class="text-xs font-normal opacity-80">{option.description}</span>
						{/if}
					</Button>
				{/each}
			</div>
			{#if multi_select}
				<Button
					type="button"
					size="sm"
					disabled={selected.length === 0}
					onclick={yield* Submit(selected)}
				>
					Answer
				</Button>
			{/if}
		{:else if item.state === "requested"}
			<form class="flex items-center gap-2" onsubmit={yield* SubmitForm(event)}>
				<Input
					bind:value={answer}
					aria-label="Answer question"
					placeholder="Type your answer"
				/>
				<Button type="submit" size="sm" disabled={answer.trim().length === 0}>Answer</Button>
			</form>
		{:else if item.resolution !== undefined}
			<p class="text-sm text-muted-foreground">{item.resolution}</p>
		{/if}
	</CardContent>
</Card>
