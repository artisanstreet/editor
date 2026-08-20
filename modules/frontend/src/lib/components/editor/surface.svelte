<script lang="ts" effect>
	import { Effect } from "effect";
	import { EditorService } from "$lib/editor/service";

	let {
		label = "\u0043ode editor",
	}: { readonly label?: string } = $props();
	let host = $state<HTMLDivElement>();

	const editor = yield* EditorService;
	const Mount = (next_host: HTMLElement) =>
		Effect.gen(function* () {
			yield* editor.Attach(next_host);
			yield* Effect.never;
		}).pipe(Effect.ensuring(editor.Detach));

	/** SER interrupts this fiber when the bound host changes or this component unmounts. */
	if (host !== undefined) yield* Mount(host);
</script>

<div
	bind:this={host}
	class="min-h-0 flex-1 overflow-hidden"
	role="region"
	aria-label={label}
></div>

