<script lang="ts">
	/**
	 * A project's square. Identity here is a colour and two letters, which is
	 * everything a 36px tile can carry and still be read across a room — the
	 * name is always beside it when the name matters.
	 */
	import { MonogramFor, type DraftProject } from "../mock";

	let {
		class: klass = "size-9 rounded-lg text-[11px]",
		project,
	}: { class?: string; project: DraftProject } = $props();
</script>

<div
	aria-hidden="true"
	class={`dz-tint grid shrink-0 place-items-center font-medium tracking-wide text-foreground-extra ${klass}`}
	style:--dz-hue={project.hue}
>
	{MonogramFor(project.name)}
</div>
