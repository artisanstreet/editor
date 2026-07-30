<script lang="ts">
	import type { RepositoryDiffCounts } from "@artisan/protocol";
	import { DiffCount } from "$lib/vcs/diff-presentation";

	type Props = {
		counts: RepositoryDiffCounts;
		label: string;
		/** Qualifies the label without competing with it, as in `origin/master (upstream)`. */
		note?: string;
		/** Whatever fact belongs beside the counts: a file tally, a commit distance. */
		trailing: string;
	};

	/**
	 * A component rather than a snippet: the effect runtime's template transform
	 * rejects type annotations on snippet parameters, which would leave these
	 * schema-derived counts unchecked.
	 */
	let { counts, label, note, trailing }: Props = $props();
</script>

<span class="text-muted-foreground">
	{label}
	{#if note !== undefined}
		<span class="text-muted-foreground/70">({note})</span>
	{/if}
</span>
<span class="whitespace-nowrap">
	<span class="text-(--diff-added)">+{DiffCount(counts.lines_added)}</span>
	<span class="text-(--diff-removed)">−{DiffCount(counts.lines_deleted)}</span>
</span>
<span class="whitespace-nowrap text-muted-foreground">{trailing}</span>
