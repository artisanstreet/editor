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
	import ConversationErrorCard from "./conversation-error-card.svelte";
	import ConversationItemView from "./conversation-item.svelte";

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
	 * Each clause is keyed by its category, so a count going 3 → 4 touches only
	 * that clause — the words rewrite in place and the digits roll — while a
	 * category appearing for the first time mounts a new clause that can animate
	 * its own arrival. A joined string would replace the whole label on either
	 * change and shift the line for both.
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

	/**
	 * The last count each clause has painted, keyed by chain and category.
	 *
	 * Like `painted_clauses`, deliberately not `$state`: it records what has
	 * been on screen so the next render knows which digits changed and which way
	 * the count moved, and a render reading it must not schedule another.
	 */
	const painted_counts = new Map<string, number>();

	type ClauseCountCell = {
		readonly char: string;
		/** Rolls only when the previous render painted something else in this column. */
		readonly entering: boolean;
		readonly outgoing: string | undefined;
		readonly place: number;
	};

	type ClauseCountPart = {
		readonly type: "count";
		readonly cells: ReadonlyArray<ClauseCountCell>;
		readonly direction: "up" | "down";
		readonly width: number;
	};

	type ClausePart = ClauseCountPart | { readonly type: "text"; readonly text: string };

	/**
	 * One reel column per digit, keyed by decimal place so 25 → 26 rolls only
	 * the ones column and 9 → 10 mounts a tens column instead of remounting the
	 * digits that stayed.
	 */
	const CountCells = (
		value: string,
		previous: string | undefined,
	): ReadonlyArray<ClauseCountCell> =>
		[...value].map((char, index) => {
			const place = value.length - 1 - index;
			const previous_char =
				previous === undefined || place >= previous.length
					? undefined
					: previous[previous.length - 1 - place];

			return {
				char,
				entering: previous !== undefined && previous_char !== char,
				outgoing:
					previous_char !== undefined && previous_char !== char ? previous_char : undefined,
				place,
			};
		});

	/**
	 * A clause split around its digit run so the count can move on its own.
	 *
	 * The words still rewrite in place; only the number is lifted into reel
	 * columns that roll in the direction of change — up for more, and down for
	 * the shrinking count a tool chain will probably never produce.
	 */
	const ClauseParts = (
		clause_id: string,
		text: string,
		count: number,
	): ReadonlyArray<ClausePart> => {
		const previous = painted_counts.get(clause_id);
		painted_counts.set(clause_id, count);

		const digits = /\d+/.exec(text);
		if (digits === null) return [{ type: "text", text }];

		/** A count of 1 paints words ("a file"), not digits: nothing to roll out. */
		const previous_digits =
			previous === undefined || previous === count
				? undefined
				: previous > 1
					? String(previous)
					: "";

		return [
			{ type: "text", text: text.slice(0, digits.index) },
			{
				cells: CountCells(digits[0], previous_digits),
				direction: previous !== undefined && count < previous ? "down" : "up",
				type: "count",
				width: digits[0].length,
			},
			{ type: "text", text: text.slice(digits.index + digits[0].length) },
		];
	};

	const GroupClauses = (
		group_id: string,
		activities: ReadonlyArray<ConversationActivityItem>,
	) => {
		const composition = GroupComposition(activities);
		const categories = composition.map(([category]) => category);

		return composition.map(([category, count], index) => {
			const clause = GetConversationActivityCountLabel(category, count);
			/** Only the opening clause is a sentence start; the rest read on from it. */
			const text =
				index === 0 ? clause.charAt(0).toUpperCase() + clause.slice(1) : `, ${clause}`;

			return {
				category,
				entering: ClauseIsNew(group_id, categories, category),
				parts: ClauseParts(`${group_id}:${category}`, text, count),
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

{#snippet clause_count(part: ClauseCountPart)}
	<!--
		A masked window the width of the digit run. The mask, not overflow, does
		the clipping: a scroll container would re-baseline the inline box and lift
		the number off the sentence's line, and the mask tracks the box through
		the width tween — so 9 → 10 reveals its new column as the cell grows
		instead of painting over the word beside it.
	-->
	<span
		class="trace-count"
		data-direction={part.direction}
		style={`--trace-count-width: ${part.width}ch`}
		>{#each part.cells as cell (cell.place)}<span class="trace-count-digit"
				>{#key cell.char}<span
						class={`trace-count-char ${cell.entering ? "trace-count-char-entering" : ""}`}
						>{cell.char}</span
					>{#if cell.outgoing !== undefined}<span
							class="trace-count-char trace-count-char-outgoing"
							aria-hidden="true">{cell.outgoing}</span
						>{/if}{/key}</span
			>{/each}</span
	>
{/snippet}

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
									><span class="trace-clause-text"
										>{#each clause.parts as part, part_index (part_index)}{#if part.type === "count"}{@render clause_count(
													part,
												)}{:else}{part.text}{/if}{/each}</span
									></span
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
								{#if diagnostic.error !== undefined}
									<!--
										A failure in Artisan custody explains itself as a card: the
										catalog names it, the code anchors it, and the provider's
										own words become the evidence line instead of the headline.
									-->
									<ConversationErrorCard
										error={diagnostic.error}
										detail={diagnostic.summary}
									/>
								{:else}
									<div
										class={`flex min-w-0 items-start gap-2 py-0.5 text-sm ${presentation.row}`}
									>
										<SeverityIcon class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
										<span class="min-w-0 break-words">{diagnostic.summary}</span>
									</div>
								{/if}
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

	/**
	 * A count ticks over like an odometer. Each digit is its own column, keyed
	 * by decimal place, so 25 → 26 rolls one glyph while the rest hold still:
	 * the incoming digit rises from below on an increment — up means higher —
	 * and the whole motion mirrors for a decrement.
	 *
	 * The window is a mask rather than `overflow` so the inline box keeps its
	 * text baseline (a scroll container synthesises one from its bottom edge),
	 * and because the mask tracks the box through the width tween — 9 → 10
	 * reveals its new column as the cell grows, clipped until there is room.
	 */
	.trace-count {
		--trace-count-shift: 1;
		--trace-count-rise: 8px;
		/** The number pop-in's bounce: the landing digit overshoots a hair and settles. */
		--trace-count-ease: cubic-bezier(0.34, 1.45, 0.64, 1);
		display: inline-flex;
		width: var(--trace-count-width);
		transition: width var(--duration-fast) var(--ease-smooth-out);
		-webkit-mask-image: linear-gradient(#000, #000);
		mask-image: linear-gradient(#000, #000);
		-webkit-mask-repeat: no-repeat;
		mask-repeat: no-repeat;
	}

	.trace-count[data-direction="down"] {
		--trace-count-shift: -1;
	}

	/** Tabular figures make every column exactly one `ch`, so width is arithmetic. */
	.trace-count-digit {
		display: inline-grid;
		justify-items: center;
		width: 1ch;
	}

	.trace-count-char {
		grid-area: 1 / 1;
		will-change: transform, opacity, filter;
	}

	.trace-count-char-entering {
		animation: trace-count-in var(--duration-fast) var(--trace-count-ease) both;
	}

	/**
	 * Resting styles are the exit's end state. A busy chain can re-render
	 * mid-roll, and stripping an animation snaps the element to its own styles —
	 * which for the outgoing glyph must mean gone, never back on top of the
	 * digit that replaced it.
	 */
	.trace-count-char-outgoing {
		opacity: 0;
		transform: translateY(calc(var(--trace-count-rise) * -1 * var(--trace-count-shift)));
		filter: blur(var(--text-swap-blur, 2px));
		animation: trace-count-out var(--duration-fast) var(--ease-smooth-out) both;
	}

	@keyframes trace-count-in {
		from {
			opacity: 0;
			transform: translateY(calc(var(--trace-count-rise) * var(--trace-count-shift)));
			filter: blur(var(--text-swap-blur, 2px));
		}
	}

	@keyframes trace-count-out {
		from {
			opacity: 1;
			transform: translateY(0);
			filter: blur(0);
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
		.trace-acc-chevron,
		.trace-count {
			transition: none !important;
		}

		.trace-clause-entering,
		.trace-count-char-entering,
		.trace-count-char-outgoing {
			animation: none !important;
		}
	}
</style>
