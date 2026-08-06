<script lang="ts" effect>
	import { dev } from "$app/environment";
	import Copy from "@tabler/icons-svelte/icons/copy";
	import PlayerTrackNext from "@tabler/icons-svelte/icons/player-track-next";
	import PlayerTrackPrev from "@tabler/icons-svelte/icons/player-track-prev";
	import { Effect } from "effect";
	import { BannerService } from "$lib/banner/service";
	import { WriteClipboardText } from "$lib/browser/clipboard";
	import { Button } from "$lib/components/ui/button";
	import {
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
	} from "$lib/components/ui/select";
	import {
		EmulatorSnapshotAt,
		EmulatorStepLabel,
		emulator_scripts,
	} from "$lib/conversation/emulator-scripts";
	import ThreadWorkspace from "$/components/thread-workspace.svelte";

	let script_id = $state(emulator_scripts[0]?.id ?? "");
	let step = $state(0);
	const banner = yield* BannerService;

	const script = $derived(
		emulator_scripts.find((candidate) => candidate.id === script_id) ?? emulator_scripts[0],
	);
	const total = $derived(script?.patches.length ?? 0);
	/** Clamped rather than reset: switching scripts should keep you near where you were. */
	const position = $derived(Math.min(step, total));
	const rebuilt = $derived(script === undefined ? undefined : EmulatorSnapshotAt(script, position));
	const snapshot = $derived(rebuilt !== undefined && "items" in rebuilt ? rebuilt : undefined);
	const failure = $derived(rebuilt !== undefined && "error" in rebuilt ? rebuilt.error : undefined);

	const SelectScript = (next_script_id: string) =>
		Effect.gen(function* () {
			script_id = next_script_id;
		});
	const PreviousStep = Effect.gen(function* () {
		step = position - 1;
	});
	const NextStep = Effect.gen(function* () {
		step = position + 1;
	});
	const UpdateStep = (event: Event & { currentTarget: HTMLInputElement }) =>
		Effect.gen(function* () {
			step = Number(event.currentTarget.value);
		});

	/**
	 * The clipboard payload is the whole reproduction: which shape, which step,
	 * and the exact snapshot the renderer was handed. A screenshot of a wrong
	 * transcript says what it looked like, not what it was given.
	 */
	const CopyStep = Effect.gen(function* () {
		if (script === undefined) return;
		yield* WriteClipboardText(
			JSON.stringify(
				{
					script: script.id,
					snapshot,
					step: position,
					step_label: EmulatorStepLabel(script, position),
					total_steps: total,
				},
				undefined,
				2,
			),
		).pipe(
			Effect.catchTag("ClipboardWriteError", () =>
				Effect.gen(function* () {
					yield* banner.error("Could not copy emulator step", {
						description: "Copy the reproduction payload manually instead.",
					});
				}),
			),
		);
	});
</script>

<!--
	Unguarded because it cannot be otherwise: `svelte:head` may not sit inside a
	block. The production build stubs this whole file, so the name never ships.
-->
<svelte:head><title>Conversation emulator</title></svelte:head>

{#if !dev}
	<div class="flex h-full items-center justify-center p-10">
		<p class="text-sm text-muted-foreground">
			This surface belongs to development tooling and is not part of this build.
		</p>
	</div>
{:else if script === undefined}
	<div class="flex h-full items-center justify-center p-10">
		<p class="text-sm text-muted-foreground">No emulator scripts are defined.</p>
	</div>
{:else}
	<div class="flex h-full min-h-0 flex-col">
		<!--
			The controls sit above the workspace rather than beside it: the transcript
			has to keep the width it has in a real thread, or the layout under review
			is not the layout that ships.
		-->
		<header class="flex shrink-0 flex-col gap-3 border-b border-border p-4">
			<div class="flex flex-wrap items-center gap-3">
				<Select type="single" value={script_id} onValueChange={yield* SelectScript(event)}>
					<SelectTrigger class="w-72" aria-label="Emulator script">
						{script.title}
					</SelectTrigger>
					<SelectContent>
						{#each emulator_scripts as candidate (candidate.id)}
							<SelectItem value={candidate.id} label={candidate.title}>
								{candidate.title}
							</SelectItem>
						{/each}
					</SelectContent>
				</Select>

				<div class="flex items-center gap-1">
					<Button
						variant="outline"
						size="icon"
						aria-label="Previous step"
						disabled={position === 0}
						onclick={yield* PreviousStep}
					>
						<PlayerTrackPrev />
					</Button>
					<Button
						variant="outline"
						size="icon"
						aria-label="Next step"
						disabled={position >= total}
						onclick={yield* NextStep}
					>
						<PlayerTrackNext />
					</Button>
				</div>

				<span class="text-xs tabular-nums text-muted-foreground">
					step {position} / {total}
				</span>

				<Button variant="outline" size="sm" class="ml-auto" onclick={yield* CopyStep}>
					<Copy data-icon="inline-start" />
					Copy step
				</Button>
			</div>

			<label class="flex flex-col gap-1">
				<span class="sr-only">Timeline</span>
				<input
					type="range"
					min="0"
					max={total}
					step="1"
					value={position}
					class="w-full accent-(--banner-info)"
					oninput={yield* UpdateStep(event)}
				/>
			</label>

			<p class="text-xs text-muted-foreground">
				<span class="text-foreground">{EmulatorStepLabel(script, position)}</span>
				— {script.description}
			</p>
		</header>

		<div class="min-h-0 flex-1">
			{#if failure !== undefined}
				<div class="flex h-full items-center justify-center p-10">
					<p class="text-sm text-(--banner-error)">
						The script violates a conversation invariant at this step: {failure}
					</p>
				</div>
			{:else if snapshot !== undefined}
				<!--
					Keyed on the script so switching shapes remounts the workspace: a
					scrub within one script must reuse the same component instance, or
					every step would look like a fresh mount and hide entry animations.
				-->
				{#key script.id}
					<ThreadWorkspace {snapshot} disabled />
				{/key}
			{/if}
		</div>
	</div>
{/if}
