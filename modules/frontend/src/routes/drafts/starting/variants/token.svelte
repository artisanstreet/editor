<script lang="ts">
	/**
	 * The smallest thing that could work.
	 *
	 * The premise: today's landing page is not wrong, it is only missing one
	 * control. A project is a property of the thread exactly like the model is,
	 * so it belongs where the model already lives — the composer's control row —
	 * and it costs no vertical space until someone opens it.
	 *
	 * This is the variant to beat. Anything more elaborate has to justify the
	 * pixels it spends over this.
	 */
	import ChevronDown from "@tabler/icons-svelte/icons/chevron-down";
	import { DraftProjects, DraftThreads, type DraftProject } from "../mock";
	import Calendar from "../pieces/calendar.svelte";
	import Composer from "../pieces/composer.svelte";
	import Monogram from "../pieces/monogram.svelte";
	import ProjectMenu from "../pieces/project-menu.svelte";
	import ThreadRow from "../pieces/thread-row.svelte";

	let selected = $state<DraftProject>(DraftProjects[0]);
	const surface_activity = DraftProjects.flatMap((project) => project.activity);
</script>

<div class="dz-vignette relative h-full overflow-hidden">
	<div class="flex h-full items-center justify-center overflow-hidden p-8 pb-48">
		<div class="w-full max-w-[50rem]">
			<section aria-label="Token usage" class="dz-enter mb-8 min-w-0 overflow-hidden">
				<Calendar class="h-24" values={surface_activity} />
			</section>

			<div class="dz-enter min-w-0" style:--dz-delay="70ms">
				{#each DraftThreads.slice(0, 4) as thread, index (thread.thread_id)}
					<div
						class="dz-enter border-b border-border last:border-b-0"
						style:--dz-delay={`${110 + index * 50}ms`}
					>
						<ThreadRow class="rounded-none py-3" show_dot={false} {thread} />
					</div>
				{/each}
			</div>
		</div>
	</div>

	<div class="absolute inset-x-0 bottom-8 mx-auto w-full max-w-[46rem] px-8">
		<div class="dz-enter" style:--dz-delay="180ms">
			<Composer placeholder="Describe a change, or ask about the code">
				{#snippet leading()}
					<ProjectMenu onselect={(project) => (selected = project)} {selected}>
						{#snippet trigger({ open, project })}
							<span
								class={`flex min-w-0 items-center gap-1.5 rounded-lg px-1.5 py-1 text-xs text-foreground transition-colors duration-150 ${open ? "bg-accent" : "hover:bg-accent"}`}
							>
								<Monogram class="size-4 rounded-[4px] text-[7px]" {project} />
								<span class="max-w-36 truncate">{project.name}</span>
								<ChevronDown class="size-3.5 shrink-0 text-muted-foreground" />
							</span>
						{/snippet}
					</ProjectMenu>
				{/snippet}
			</Composer>
		</div>
	</div>
</div>
