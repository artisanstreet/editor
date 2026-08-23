<script lang="ts">
	import type { HTMLImgAttributes } from "svelte/elements";
	import logo from "./logo.svg";

	let { alt = "", ...attributes }: Omit<HTMLImgAttributes, "src"> = $props();
</script>

<img src={logo} {alt} {...attributes} />
