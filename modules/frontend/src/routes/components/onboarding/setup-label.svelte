<script lang="ts">
	import { tick } from "svelte";

	let { email, label }: { email?: string; label: string } = $props();
	let element = $state<HTMLElement>();
	let initialized = false;
	let rendered_email = $state<string | undefined>();
	let rendered_label = $state("");

	$effect(() => {
		const next_email = email;
		const next_label = label;
		if (!initialized) {
			initialized = true;
			rendered_email = next_email;
			rendered_label = next_label;
			return;
		}
		if (next_email === rendered_email && next_label === rendered_label) return;
		const node = element;
		if (node === undefined) {
			rendered_email = next_email;
			rendered_label = next_label;
			return;
		}
		const duration =
			Number.parseFloat(
				getComputedStyle(document.documentElement).getPropertyValue("--text-swap-dur"),
			) || 150;
		node.classList.add("is-exit");
		const timeout = window.setTimeout(async () => {
			rendered_email = next_email;
			rendered_label = next_label;
			await tick();
			node.classList.remove("is-exit");
			node.classList.add("is-enter-start");
			void node.offsetHeight;
			node.classList.remove("is-enter-start");
		}, duration);
		return () => window.clearTimeout(timeout);
	});
</script>

<span bind:this={element} class="t-text-swap">
	{rendered_label}{#if rendered_email !== undefined}
		{" "}<span class="text-background/85">{rendered_email}</span>
	{/if}
</span>
