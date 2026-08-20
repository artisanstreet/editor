<script lang="ts" effect>
	import BotId from "@tabler/icons-svelte/icons/bot-id";
	import type { ThreadSessionSnapshot } from "@artisan/protocol";
	import { Effect, Stream } from "effect";
	import { dispatch_model_presentation } from "$lib/engine/dispatch-presentation";
	import { EngineMarkClass } from "$lib/engine/presentation";
	import type { OrchestrationRosterEntry } from "$lib/orchestration/roster";
	import {
		ThreadSessionProjection,
		type ThreadSessionProjectionState,
	} from "$lib/thread-interaction/session-projection";
	import HoverPill, { type PillHover } from "./hover-pill.svelte";

	let {
		entries,
		hover,
		oninspect,
		thread_id,
	}: {
		readonly entries: ReadonlyArray<OrchestrationRosterEntry>;
		readonly hover: PillHover;
		readonly oninspect: (entry: OrchestrationRosterEntry) => Effect.Effect<void>;
		readonly thread_id: string | undefined;
	} = $props();

	/**
	 * Assignments carry the model choice but not effort or speed; those are the
	 * thread policy's, applied at dispatch. The route publishes its authoritative
	 * aggregate into this retained projection; the inspector never issues an RPC.
	 */
	const session_projection = yield* ThreadSessionProjection;
	let session = $state.raw<ThreadSessionSnapshot | undefined>();
	const LoadCurrentSession = (next_thread_id: string | undefined) =>
		Effect.gen(function* () {
			session =
				next_thread_id === undefined
					? undefined
					: yield* session_projection.Current(next_thread_id);
		});
	yield* LoadCurrentSession(thread_id);
	const ApplyProjection = (state: ThreadSessionProjectionState) =>
		Effect.gen(function* () {
			session = thread_id === undefined ? undefined : state.sessions.get(thread_id);
		});
	yield* session_projection.Changes.pipe(
		Stream.runForEach(ApplyProjection),
		Effect.forkScoped,
	);
	const policy = $derived(session?.policy);

	const terminal_state = (state: OrchestrationRosterEntry["state"]) =>
		state === "complete" || state === "failed";
	const state_label = (state: OrchestrationRosterEntry["state"]) =>
		state === "failed" ? "Agent failed" : "Agent completed";
	const state_tone = (state: OrchestrationRosterEntry["state"]) =>
		state === "failed"
			? "--state-dot-tone: var(--destructive)"
			: "--state-dot-tone: var(--unread)";
</script>

<section class="min-h-0" aria-labelledby="thread-agents-heading">
	<h2 id="thread-agents-heading" class="relative px-2 pt-2 pb-1 text-sm font-medium text-foreground">
		Agents
	</h2>
	<div class="relative">
		<HoverPill {hover} />
		<ul class="@container flex min-w-0 flex-col" aria-label="Active agents">
			{#each entries as entry (`${entry.group_id}:${entry.agent_id}`)}
				{@const dispatch = dispatch_model_presentation(policy, entry.engine_id, entry.profile)}
				<li>
					<button
						type="button"
						class="relative flex w-full min-w-0 items-center justify-between gap-4 rounded-lg px-2 py-2 text-left text-sm outline-none"
						onclick={yield* oninspect(entry)}
						onpointerenter={hover.move}
						onpointermove={hover.move}
						onfocusin={hover.move}
					>
						<span class="flex min-w-0 flex-1 items-center gap-2">
							<BotId class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
							<!-- Ellipsis, not a fade: a fading last letter reads as a rendering fault. -->
							<span class="min-w-0 truncate text-foreground">
								{entry.display_name}
							</span>
						</span>
						<span class="flex shrink-0 items-center gap-2">
							{#if dispatch !== undefined}
								<!--
									What the agent runs on, spelled out as far as the row can
									afford. Each qualifier is dropped whole rather than
									truncated — a clipped "Extra Hi" reads as a different
									setting — and below the last tier the model collapses to
									its lab's mark, which stays recognisable at any width.
									Muted throughout, model included: this is provenance, not a
									control, and the picker's amber and gradient speed tints are
									its own trigger's vocabulary — anything brighter here is the
									loudest thing in the card and says nothing extra.
								-->
								{@const MarkIcon = dispatch.mark.icon}
								<span
									class="flex items-center gap-1 whitespace-nowrap text-xs text-muted-foreground"
									aria-label={dispatch.aria_label}
								>
									<MarkIcon
										class={`${EngineMarkClass(dispatch.mark, "size-3.5")} @min-[13rem]:hidden`}
										aria-hidden="true"
									/>
									<span class="hidden @min-[13rem]:inline">
										{dispatch.model_name}
									</span>
									{#if dispatch.effort_label !== undefined}
										<span class="hidden @min-[16rem]:inline">{dispatch.effort_label}</span>
									{/if}
									{#if dispatch.speed !== undefined}
										<span class="hidden @min-[19rem]:inline">{dispatch.speed.label}</span>
									{/if}
								</span>
							{/if}
							{#if terminal_state(entry.state)}
								<span
									class="state-dot size-1.5 shrink-0 rounded-full"
									style={state_tone(entry.state)}
									aria-label={state_label(entry.state)}
								></span>
							{/if}
						</span>
					</button>
				</li>
			{/each}
		</ul>
	</div>
</section>
