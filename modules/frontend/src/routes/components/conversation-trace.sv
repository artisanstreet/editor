<script lang="ts" effect>
	import {
		GetConversationActivityCategory,
		GetConversationActivityCountLabel,
		GetConversationActivityPresentation,
		type ConversationActivityCategory,
		type ConversationItem,
	} from "@artisan/protocol";
	import AlertTriangle from "@tabler/icons-svelte/icons/alert-triangle";
	import Bug from "@tabler/icons-svelte/icons/bug";
	import CircleX from "@tabler/icons-svelte/icons/circle-x";
	import FilePencil from "@tabler/icons-svelte/icons/file-pencil";
	import FileSearch from "@tabler/icons-svelte/icons/file-search";
	import FileText from "@tabler/icons-svelte/icons/file-text";
	import FileX from "@tabler/icons-svelte/icons/file-x";
	import ListDetails from "@tabler/icons-svelte/icons/list-details";
	import Terminal2 from "@tabler/icons-svelte/icons/terminal-2";
	import Tool from "@tabler/icons-svelte/icons/tool";
	import WorldSearch from "@tabler/icons-svelte/icons/world-search";
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import { Effect } from "effect";
	import { conversation_activity_is_live } from "$lib/conversation/activity-status";
	import { conversation_diagnostics_enabled } from "$lib/conversation/diagnostics";
	import { ShimmerText } from "$lib/components/ui/shimmer-text";
	import { PresentShellCommand } from "$lib/conversation/shell-command";
	import {
		make_conversation_trace_segments,
		type ConversationActivityItem,
	} from "$lib/conversation/trace";
	import ConversationItemView from "./conversation-item.sv";

	let {
		failed = false,
		items,
		work_active = false,
	}: {
		/** Failed work must explain itself: diagnostics render open and unmuted. */
		failed?: boolean;
		items: ReadonlyArray<ConversationItem>;
		/**
		 * A stale provider item cannot keep animating after its owning work has
		 * settled, and reasoning outlives its own completion only while that work
		 * still runs.
		 */
		work_active?: boolean;
	} = $props();
	let open_groups = $state<Record<string, boolean>>({});

	const segments = $derived(
		make_conversation_trace_segments(
			items,
			$conversation_diagnostics_enabled,
			failed,
			work_active,
		),
	);

	/**
	 * What the chain is made of, in the order those kinds first appeared. A count
	 * per category is what lets the header describe the work rather than measure
	 * it: "Used 7 tools" is a number, "Ran 4 commands, edited 2 files" is an
	 * account of the same seven.
	 */
	const GroupComposition = (activities: ReadonlyArray<ConversationActivityItem>) => {
		const counts = new Map<ConversationActivityCategory, number>();
		for (const activity of activities) {
			const category = GetConversationActivityCategory(activity.kind);
			counts.set(category, (counts.get(category) ?? 0) + 1);
		}
		return [...counts];
	};

	/**
	 * The header's clauses, kept as parts rather than one joined string.
	 *
	 * Each clause is keyed by its category, so a count going 3 → 4 rewrites that
	 * clause's text in place — no element is replaced and nothing around it moves
	 * — while a category appearing for the first time mounts a new clause that can
	 * animate its own arrival. A joined string would replace the whole label on
	 * either change and shift the line for both.
	 */
	/**
	 * Which clauses of a chain have already been on screen.
	 *
	 * A clause animates open only if its chain was already rendered without it.
	 * Animating the ones a chain arrives with meant the header mounted as an icon
	 * and a chevron either side of nothing while the label grew — a summary that
	 * reads as empty for the length of its own entrance. Deliberately not `$state`:
	 * this records what has been painted, and a render reading it must not be a
	 * render that schedules another.
	 */
	const painted_clauses = new Map<string, Set<ConversationActivityCategory>>();

	const ClauseIsNew = (
		group_id: string,
		categories: ReadonlyArray<ConversationActivityCategory>,
		category: ConversationActivityCategory,
	) => {
		const painted = painted_clauses.get(group_id);
		if (painted === undefined) {
			painted_clauses.set(group_id, new Set(categories));
			return false;
		}
		if (painted.has(category)) return false;
		painted.add(category);
		return true;
	};

	const GroupClauses = (
		group_id: string,
		activities: ReadonlyArray<ConversationActivityItem>,
	) => {
		const composition = GroupComposition(activities);
		const categories = composition.map(([category]) => category);

		return composition.map(([category, count], index) => {
			const clause = GetConversationActivityCountLabel(category, count);

			return {
				category,
				entering: ClauseIsNew(group_id, categories, category),
				/** Only the opening clause is a sentence start; the rest read on from it. */
				text: index === 0 ? clause.charAt(0).toUpperCase() + clause.slice(1) : `, ${clause}`,
			};
		});
	};

	/** Counted in the header so the size of a collapsed group is readable without expanding it. */
	const GroupLabel = (activities: ReadonlyArray<ConversationActivityItem>) =>
		GroupComposition(activities)
			.map(([category, count], index) => {
				const clause = GetConversationActivityCountLabel(category, count);

				return index === 0 ? clause.charAt(0).toUpperCase() + clause.slice(1) : `, ${clause}`;
			})
			.join("");

	/**
	 * The head icon names the work when the chain did one kind of thing, and
	 * falls back to a list when it did more — the one case where no single tool
	 * icon would be honest about what is collapsed underneath it. A list is what
	 * the header is at that point: several counted things behind one line.
	 */
	const CategoryIcon = (category: ConversationActivityCategory) => {
		if (category === "command" || category === "test" || category === "typecheck")
			return Terminal2;
		if (category === "file_read") return FileText;
		if (category === "file_edit") return FilePencil;
		if (category === "file_delete") return FileX;
		if (category === "file_search") return FileSearch;
		if (category === "web_search") return WorldSearch;
		return Tool;
	};

	const GroupIcon = (activities: ReadonlyArray<ConversationActivityItem>) => {
		const composition = GroupComposition(activities);
		const only = composition.length === 1 ? composition[0] : undefined;

		return only === undefined ? ListDetails : CategoryIcon(only[0]);
	};

	/**
	 * Whether anything in this chain is still running. It decides how the header
	 * is painted, never what it says: the header used to front the running
	 * command, which meant a line that rewrote itself wholesale on every call and
	 * read as a different subject each time. What the chain has done so far is the
	 * one thing that stays true across the whole run, so the summary always holds
	 * and the shimmer alone carries that it is still going.
	 */
	const GroupIsLive = (activities: ReadonlyArray<ConversationActivityItem>) =>
		work_active && activities.some(conversation_activity_is_live);

	const ToggleGroup = (id: string) =>
		Effect.gen(function* () {
		open_groups[id] = !open_groups[id];
		});

	/**
	 * One voice per severity tier. Failures are the only tier allowed to read as
	 * an error: warnings carry the caution tone and quiet diagnostics stay muted,
	 * so a wall of usage reports can never impersonate a failure again.
	 */
	const severity_presentation = {
		error: {
			head: "text-destructive hover:text-destructive",
			icon: CircleX,
			label: "Failures",
			row: "text-destructive",
		},
		warning: {
			head: "text-warning hover:text-warning",
			icon: AlertTriangle,
			label: "Warnings",
			row: "text-warning",
		},
		info: {
			head: "text-muted-foreground hover:text-foreground",
			icon: Bug,
			label: "Diagnostics",
			row: "text-muted-foreground",
		},
	} as const;
</script>

{#if segments.length > 0}
	<div class="flex flex-col gap-5">
		{#each segments as segment (segment.id)}
			{#if segment.type === "item"}
				<ConversationItemView item={segment.item} />
			{:else if segment.type === "activity_group"}
				{@const open = open_groups[segment.id] ?? false}
				{@const live = GroupIsLive(segment.items)}
				{@const clauses = GroupClauses(segment.id, segment.items)}
				{@const HeadIcon = GroupIcon(segment.items)}
				<div
					class="trace-acc flex flex-col"
					data-open={open}
					data-state={open ? "open" : "closed"}
				>
					<button
						type="button"
						class="trace-acc-head flex w-fit max-w-full cursor-pointer items-center gap-2 py-0.5 text-base text-muted-foreground transition-colors duration-150 hover:text-foreground motion-reduce:transition-none"
						aria-expanded={open}
						onclick={yield* ToggleGroup(segment.id)}
					>
						<HeadIcon class="size-4 shrink-0" aria-hidden="true" />
						<!--
							Never remounted. The summary is the same sentence for the life of
							the chain, so a count going 3 → 4 rewrites one clause's text in
							place and only a genuinely new kind of work mounts anything — which
							is the one change that earns an entrance.
						-->
						<!--
							One element whether or not the chain is live. Branching to a plain
							span when it settles replaced the subtree, and a chain goes quiet
							between every call — so each gap remounted the clauses and replayed
							their grow-from-nothing, which is the label reading "Read 2 files,"
							with nothing after it.
						-->
						<!--
							Inherit, never a color of its own. ShimmerText's variant paints
							`text-foreground`, which pinned the label while the button's
							hover recolored only the icon and chevron around it; inheriting
							keeps the whole header muted at rest and lifts it as one word on
							hover.
						-->
						<ShimmerText
							active={live}
							class="trace-head-label min-w-0 flex-1 truncate text-inherit"
						>
							{#each clauses as clause (clause.category)}
								<span
									class={`trace-clause ${clause.entering ? "trace-clause-entering" : ""}`}
									><span class="trace-clause-text">{clause.text}</span></span
								>
							{/each}
						</ShimmerText>
						<!-- The chevron belongs to the label: gap-1 from it, like the work-session header. -->
						<span class="trace-acc-chevron -ml-1 flex shrink-0">
							<ChevronRight class="size-3.5" aria-hidden="true" />
						</span>
					</button>

					<div class="trace-acc-panel">
						<div class="trace-acc-panel-inner flex flex-col gap-1 pt-1">
							{#each segment.items as activity (activity.id)}
								{@const ActivityIcon = CategoryIcon(
									GetConversationActivityCategory(activity.kind),
								)}
								<div class="flex w-full min-w-0 flex-row items-center gap-2 py-0.5 text-base text-muted-foreground">
									<ActivityIcon class="size-4 shrink-0" aria-hidden="true" />
									{#if activity.kind === "terminal_activity" && activity.detail !== undefined}
										<!-- The monospace face already says "shell"; a prompt glyph is noise. -->
										<span class="trace-command-label min-w-0 flex-1 font-mono text-sm">
											{PresentShellCommand(activity.detail)}
										</span>
									{:else}
										<!-- A row without its own detail still reads as normalized work, not a raw provider label. -->
										<span class="min-w-0 truncate">
											{activity.detail ??
												GetConversationActivityPresentation(activity).label}
										</span>
									{/if}
								</div>
							{/each}
						</div>
					</div>
				</div>
			{:else}
				{@const presentation = severity_presentation[segment.severity]}
				{@const SeverityIcon = presentation.icon}
				{@const alerting = failed && segment.severity === "error"}
				<!-- Only failures of a failed run open themselves; every other tier waits to be asked. -->
				{@const open = open_groups[segment.id] ?? alerting}
				<div
					class="trace-acc flex flex-col"
					data-open={open}
					data-state={open ? "open" : "closed"}
					role={alerting ? "alert" : undefined}
				>
					<button
						type="button"
						class={`trace-acc-head flex w-fit cursor-pointer items-center gap-2 py-0.5 text-base transition-colors duration-150 motion-reduce:transition-none ${presentation.head}`}
						aria-expanded={open}
						onclick={yield* ToggleGroup(segment.id)}
					>
						<SeverityIcon class="size-4" aria-hidden="true" />
						<span>{presentation.label}</span>
						<span class="trace-acc-chevron -ml-1 flex">
							<ChevronRight class="size-3.5" aria-hidden="true" />
						</span>
					</button>

					<div class="trace-acc-panel">
						<div class="trace-acc-panel-inner flex flex-col gap-1 pt-1">
							{#each segment.items as diagnostic (diagnostic.id)}
								<div
									class={`flex min-w-0 items-start gap-2 py-0.5 text-sm ${presentation.row}`}
								>
									<SeverityIcon class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
									<span class="min-w-0 break-words">{diagnostic.summary}</span>
								</div>
							{/each}
						</div>
					</div>
				</div>
			{/if}
		{/each}
	</div>
{/if}

<style>
	/**
	 * Digits hold one width, so a count going 3 → 4 rewrites the glyph and moves
	 * nothing after it. Proportional figures would nudge the rest of the sentence
	 * sideways on a change that is meant to be invisible.
	 */
	.trace-head-label {
		font-variant-numeric: tabular-nums;
	}

	/**
	 * A new clause grows the label open rather than appearing at full width.
	 *
	 * `width` cannot tween to `auto`, so the clause is a one-column grid and the
	 * track carries the motion from `0fr` to its content — the horizontal twin of
	 * the disclosure panel below. It is a mount animation rather than a
	 * transition because a clause has no previous state to leave: it did not
	 * exist until the chain did that kind of work for the first time.
	 *
	 * Only clauses added to a chain already on screen animate. A chain's opening
	 * clauses arrive already open, or the header would mount as an icon and a
	 * chevron either side of nothing for the length of its own entrance.
	 */
	.trace-clause {
		display: inline-grid;
		grid-template-columns: 1fr;
	}

	.trace-clause-entering {
		animation: trace-clause-in var(--duration-fast) var(--ease-smooth-out) both;
	}

	.trace-clause-text {
		overflow: hidden;
		white-space: nowrap;
	}

	@keyframes trace-clause-in {
		from {
			grid-template-columns: 0fr;
			opacity: 0;
			filter: blur(var(--text-swap-blur, 2px));
		}
	}

	.trace-acc-panel {
		display: grid;
		grid-template-rows: 0fr;
		transition: grid-template-rows 250ms cubic-bezier(0.22, 1, 0.36, 1);
	}

	.trace-acc[data-open="true"] .trace-acc-panel {
		grid-template-rows: 1fr;
	}

	.trace-acc-panel-inner {
		overflow: hidden;
		opacity: 0;
		filter: blur(2px);
		transition:
			opacity 250ms cubic-bezier(0.22, 1, 0.36, 1),
			filter 250ms cubic-bezier(0.22, 1, 0.36, 1);
	}

	.trace-acc[data-open="true"] .trace-acc-panel-inner {
		opacity: 1;
		filter: blur(0);
	}

	@property --trace-command-fade-end {
		syntax: "<length>";
		inherits: false;
		initial-value: 0px;
	}

	/** The mask belongs only to clipped command text; icons and disclosure stay crisp. */
	.trace-command-label {
		--trace-command-fade-size: 1.5rem;
		overflow: hidden;
		white-space: nowrap;
		-webkit-mask-image: linear-gradient(
			to right,
			black calc(100% - var(--trace-command-fade-end)),
			transparent
		);
		mask-image: linear-gradient(
			to right,
			black calc(100% - var(--trace-command-fade-end)),
			transparent
		);
	}

	/** The existing scroll-fade technique keeps a fitting command fully opaque. */
	@supports (animation-timeline: scroll()) {
		.trace-command-label {
			animation: trace-command-fade-end linear both;
			animation-timeline: scroll(self inline);
			animation-range: 0 100%;
		}
	}

	@keyframes trace-command-fade-end {
		from {
			--trace-command-fade-end: var(--trace-command-fade-size);
		}
		to {
			--trace-command-fade-end: 0px;
		}
	}

	.trace-acc-chevron {
		transform: rotate(0deg);
		transform-origin: center;
		transition: transform 250ms cubic-bezier(0.22, 1, 0.36, 1);
	}

	.trace-acc[data-open="true"] .trace-acc-chevron {
		transform: rotate(90deg);
	}

	@media (prefers-reduced-motion: reduce) {
		.trace-acc-panel,
		.trace-acc-panel-inner,
		.trace-acc-chevron {
			transition: none !important;
		}

		.trace-clause-entering {
			animation: none !important;
		}
	}
</style>
