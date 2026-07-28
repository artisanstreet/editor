<script lang="ts">
	import Anchor from "./anchor.sv";

	let { alt, src }: { alt?: string; src?: string } = $props();

	const label = $derived(alt !== undefined && alt.length > 0 ? alt : "image");
</script>

<!--
	Images in assistant markdown are never auto-fetched: a remote image URL is
	a silent exfiltration and tracking channel for prompt-injected output. The
	image renders as a protocol-vetted link the user can choose to open.
-->
<Anchor href={src}>{label}</Anchor>
