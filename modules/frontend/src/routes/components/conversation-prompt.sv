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
		item: Extract<ConversationItem, { type: "plan" | "question" }>;
		onquestion?: (
			question_id: string,
			answer: string,
		) => Effect.Effect<void, { readonly message: string }>;
	} = $props();

	let answer = $state("");
	const label = $derived(
		item.type === "plan"
			? "Plan"
			: item.state === "requested"
				? "Question"
				: "Answered",
	);
	const text = $derived(item.type === "plan" ? item.entries.map((entry) => entry.text).join("\n") : item.prompt);

	const SubmitAnswer = (value: string) =>
		Effect.gen(function* () {
			if (item.type !== "question" || value.length === 0 || onquestion === undefined) return;
			yield* onquestion(item.interaction_id, value);
			answer = "";
		});

	const SubmitForm = (event: SubmitEvent) =>
		Effect.gen(function* () {
			yield* RunBrowserDom(() => event.preventDefault());
			yield* SubmitAnswer(answer.trim());
		});
</script>

<Card size="sm" class="max-w-(--prose-body-width) py-3">
	<CardContent class="space-y-3">
		<Badge variant="outline">{label}</Badge>
		<p class="whitespace-pre-wrap text-base leading-7">{text}</p>

		{#if item.type === "question" && item.state === "requested"}
			<form
				class="flex items-center gap-2"
				onsubmit={yield* SubmitForm(event)}
			>
				<Input
					bind:value={answer}
					aria-label="Answer question"
					placeholder="Type your answer"
				/>
				<Button type="submit" size="sm" disabled={answer.trim().length === 0}>Answer</Button>
			</form>
		{:else if item.type === "question" && item.resolution !== undefined}
			<p class="text-sm text-muted-foreground">{item.resolution}</p>
		{/if}
	</CardContent>
</Card>
