<script lang="ts" effect>
	/**
	 * What the composer's last send or attachment refused.
	 *
	 * This used to be a toast. The notification service it went through was
	 * removed and its call sites were left empty, which turned every refused send
	 * into nothing happening at all — the text stayed in the box with no reason
	 * given, so the only available reading was that the message had been sent.
	 *
	 * It reports directly over the composer rather than in a corner of the
	 * window: the text it refused is still sitting in the box below, and the two
	 * belong to the same glance.
	 */
	import { Effect } from "effect";
	import { Button } from "$lib/components/ui/button";
	import type { ComposerActionFailure } from "$lib/composer/action-failure";

	let {
		failure,
		ondismiss,
	}: {
		/** The refusal to answer for, or nothing while the last attempt stands. */
		readonly failure: ComposerActionFailure | undefined;
		readonly ondismiss: Effect.Effect<void>;
	} = $props();
</script>

{#if failure !== undefined}
	<div class="prose-column flex w-full max-w-(--prose-width) justify-center">
		<div
			class="pointer-events-auto flex w-full items-start gap-3 rounded-xl border border-destructive/40 bg-background px-4 py-3 shadow-lg"
			role="alert"
		>
			<div class="flex min-w-0 flex-1 flex-col gap-0.5">
				<span class="text-sm font-medium text-foreground">{failure.title}</span>
				<span class="text-sm [overflow-wrap:anywhere] text-muted-foreground"
					>{failure.description}</span
				>
			</div>
			<Button
				variant="ghost"
				size="sm"
				class="shrink-0 text-muted-foreground hover:text-foreground"
				onclick={yield* ondismiss}
			>
				Dismiss
			</Button>
		</div>
	</div>
{/if}
