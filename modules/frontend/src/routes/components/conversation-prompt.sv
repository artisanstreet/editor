<script lang="ts">
	import type { ConversationItem } from "@artisan/protocol";
	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Card, CardContent } from "$lib/components/ui/card";
	import { Input } from "$lib/components/ui/input";

	let {
		item,
		onapproval,
		onquestion,
	}: {
		item: Extract<ConversationItem, { type: "plan" | "approval" | "question" }>;
		onapproval?: (approval_id: string, approved: boolean) => void;
		onquestion?: (question_id: string, answer: string) => void;
	} = $props();

	let answer = $state("");
	const label = $derived(
		item.type === "plan"
			? "Plan"
			: item.type === "approval"
				? item.state === "requested"
					? "Approval needed"
					: item.state
				: item.state === "requested"
					? "Question"
					: "Answered",
	);
	const text = $derived(item.type === "plan" ? item.entries.map((entry) => entry.text).join("\n") : item.prompt);

	const SubmitAnswer = () => {
		const value = answer.trim();
		if (item.type !== "question" || value.length === 0) return;
		onquestion?.(item.interaction_id, value);
		answer = "";
	};
</script>

<Card size="sm" class="max-w-2xl py-3">
	<CardContent class="space-y-3">
		<Badge variant="outline">{label}</Badge>
		<p class="whitespace-pre-wrap text-base leading-7">{text}</p>

		{#if item.type === "approval" && item.state === "requested"}
			<div class="flex flex-wrap items-center gap-2">
				<Button size="sm" onclick={() => onapproval?.(item.interaction_id, true)}>
					Approve
				</Button>
				<Button
					size="sm"
					variant="outline"
					onclick={() => onapproval?.(item.interaction_id, false)}
				>
					Deny
				</Button>
			</div>
		{:else if item.type === "question" && item.state === "requested"}
			<form
				class="flex items-center gap-2"
				onsubmit={(event) => {
					event.preventDefault();
					SubmitAnswer();
				}}
			>
				<Input
					bind:value={answer}
					aria-label="Answer question"
					placeholder="Type your answer"
				/>
				<Button type="submit" size="sm" disabled={answer.trim().length === 0}>Answer</Button>
			</form>
		{:else if item.type !== "plan" && item.resolution !== undefined}
			<p class="text-sm text-muted-foreground">{item.resolution}</p>
		{/if}
	</CardContent>
</Card>
