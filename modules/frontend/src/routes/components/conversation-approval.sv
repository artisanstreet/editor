<script lang="ts" effect>
	import type { ConversationItem } from "@artisan/protocol";
	import { Effect } from "effect";
	import Check from "@tabler/icons-svelte/icons/check";
	import CircleCheck from "@tabler/icons-svelte/icons/circle-check";
	import CircleX from "@tabler/icons-svelte/icons/circle-x";
	import FileDiff from "@tabler/icons-svelte/icons/file-diff";
	import Terminal2 from "@tabler/icons-svelte/icons/terminal-2";
	import X from "@tabler/icons-svelte/icons/x";
	import { BannerService } from "$lib/banner/service";
	import { Button } from "$lib/components/ui/button";
	import { GetApprovalPresentation } from "$lib/conversation/approval-presentation";

	const banner = yield* BannerService;
	let {
		item,
		onapproval,
	}: {
		item: Extract<ConversationItem, { type: "approval" }>;
		onapproval?: (
			approval_id: string,
			approved: boolean,
		) => Effect.Effect<void, { readonly message: string }>;
	} = $props();

	let submitted_decision = $state<boolean | undefined>();
	const presentation = $derived(GetApprovalPresentation(item));
	const title_id = $derived(`approval-title-${item.ordinal}`);
	const pending_label = $derived(
		submitted_decision === false
			? "Denying…"
			: presentation.kind === "command"
				? "Starting…"
				: presentation.kind === "file_change"
					? "Applying…"
					: "Approving…",
	);

	const SubmitDecision = (approved: boolean) =>
		Effect.gen(function* () {
			if (submitted_decision !== undefined || onapproval === undefined) return;
			submitted_decision = approved;
			yield* onapproval(item.interaction_id, approved).pipe(
				Effect.catch((error) =>
					Effect.gen(function* () {
						submitted_decision = undefined;
						yield* banner.error("Could not respond to approval", {
							description: error.message,
						});
					}),
				),
			);
		});
</script>

{#if item.state === "requested"}
	<section
		class="card max-w-2xl rounded-2xl p-4"
		aria-busy={submitted_decision !== undefined}
		aria-labelledby={title_id}
	>
		<div class="flex items-start gap-3">
			<div
				class="mt-0.5 flex size-9 shrink-0 items-center justify-center rounded-xl bg-foreground/5 text-muted-foreground ring-1 ring-foreground/10"
			>
				{#if presentation.kind === "command"}
					<Terminal2 class="size-4.5" aria-hidden="true" />
				{:else}
					<FileDiff class="size-4.5" aria-hidden="true" />
				{/if}
			</div>

			<div class="min-w-0 flex-1">
				<h3 id={title_id} class="text-base font-medium">
					{presentation.title}
				</h3>
				{#if presentation.description !== undefined}
					<p class="mt-1 text-sm leading-6 text-muted-foreground">
						{presentation.description}
					</p>
				{/if}

				{#if presentation.command !== undefined}
					<div
						class="mt-3 overflow-hidden rounded-xl bg-background/50 ring-1 ring-foreground/10"
					>
						<pre
							class="overflow-x-auto px-3 py-2.5 font-mono text-sm leading-6 text-foreground"><code>{presentation.command}</code></pre
						>
						{#if presentation.cwd !== undefined}
							<p
								class="truncate border-t border-foreground/10 px-3 py-2 font-mono text-xs text-muted-foreground"
								title={presentation.cwd}
							>
								{presentation.cwd}
							</p>
						{/if}
					</div>
				{/if}

				<div class="mt-4 flex flex-wrap items-center gap-2">
					<Button
						size="sm"
						disabled={onapproval === undefined || submitted_decision !== undefined}
						onclick={yield* SubmitDecision(true)}
					>
						<Check data-icon="inline-start" aria-hidden="true" />
						{submitted_decision === true ? pending_label : presentation.approve_label}
					</Button>
					<Button
						size="sm"
						variant="outline"
						disabled={onapproval === undefined || submitted_decision !== undefined}
						onclick={yield* SubmitDecision(false)}
					>
						<X data-icon="inline-start" aria-hidden="true" />
						{submitted_decision === false ? pending_label : "Deny"}
					</Button>
				</div>
			</div>
		</div>
	</section>
{:else}
	<div
		class="flex max-w-2xl items-center gap-2 text-sm text-muted-foreground"
		role="status"
	>
		{#if item.state === "approved"}
			<CircleCheck class="size-4 shrink-0" aria-hidden="true" />
		{:else}
			<CircleX class="size-4 shrink-0" aria-hidden="true" />
		{/if}
		<span class="shrink-0">{presentation.title}</span>
		{#if presentation.command !== undefined}
			<code class="min-w-0 truncate font-mono text-xs text-muted-foreground/80">
				{presentation.command}
			</code>
		{/if}
	</div>
{/if}
