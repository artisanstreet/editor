<script lang="ts" effect>
	import {
		GetConversationActivityCategory,
		GetConversationActivityCategoryLabel,
		GetConversationActivityGroupPresentation,
		GetConversationActivityPresentation,
		type ConversationActivityCategory,
		type ConversationItem,
	} from "@artisan/protocol";
	import AlertTriangle from "@tabler/icons-svelte/icons/alert-triangle";
	import Bug from "@tabler/icons-svelte/icons/bug";
	import CircleX from "@tabler/icons-svelte/icons/circle-x";
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import FilePencil from "@tabler/icons-svelte/icons/file-pencil";
	import FileSearch from "@tabler/icons-svelte/icons/file-search";
	import FileText from "@tabler/icons-svelte/icons/file-text";
	import FileX from "@tabler/icons-svelte/icons/file-x";
	import ListDetails from "@tabler/icons-svelte/icons/list-details";
	import Terminal2 from "@tabler/icons-svelte/icons/terminal-2";
	import Tool from "@tabler/icons-svelte/icons/tool";
	import WorldSearch from "@tabler/icons-svelte/icons/world-search";
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
		 * Owning work's liveness, which alone may let a group keep shimmering: a
		 * provider can leave a settled run's last activity looking open forever.
		 */
		work_active?: boolean;
	} = $props();
	let open_groups = $state<Record<string, boolean>>({});

	const segments = $derived(
		make_conversation_trace_segments(items, $conversation_diagnostics_enabled, failed),
	);

	/**
	 * What the chain is made of, in the order those kinds first appeared. A count
	 * per category is what lets the header describe the work rather than measure
	 * it: "Used 7 tools" is a number, "Ran 4 commands, edited 2 files" is an
	 * account of the same seven.
	 */
	const GroupComposition = (activities: ReadonlyArray<ConversationActivityItem>) => {
		const members = new Map<ConversationActivityCategory, Array<ConversationActivityItem>>();
		for (const activity of activities) {
			const category = GetConversationActivityCategory(activity.kind);
			const current = members.get(category) ?? [];
			current.push(activity);
			members.set(category, current);
		}
		return [...members];
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

		return composition.map(([category, members], index) => {
			const presentation = GetConversationActivityGroupPresentation(category, members);
			const clause = presentation.label;
			const text =
				index === 0 ? clause.charAt(0).toUpperCase() + clause.slice(1) : `, ${clause}`;

			return {
				category,
				entering: ClauseIsNew(group_id, categories, category),
				live: work_active && members.some(conversation_activity_is_live),
				parts: ClauseParts(`${group_id}:${category}`, text, presentation.count),
			};
		});
	};

	/**
	 * A homogeneous chain is represented by its tool category. Mixed work uses
	 * the group glyph because no single category can honestly name its contents.
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
						class="trace-count-char" data-entering={cell.entering ? "true" : undefined}
						>{cell.char}</span
					>{#if cell.outgoing !== undefined}<span
							class="trace-count-char" data-outgoing="true"
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
				{@const clauses = GroupClauses(segment.id, segment.items)}
				{@const HeadIcon = GroupIcon(segment.items)}
				<div
					class="t-acc group/trace-acc flex flex-col"
					data-open={open}
					data-state={open ? "open" : "closed"}
				>
					<button
						type="button"
						class="trace-acc-head flex w-fit max-w-full cursor-pointer items-center gap-2 py-0.5 text-base text-muted-foreground transition-colors duration-150 hover:text-foreground group-data-[open=true]/trace-acc:text-foreground motion-reduce:transition-none"
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
							The shimmer is per clause, not per chain. Liveness decides how a
							clause is painted, never what the header says — and a clause only
							claims the work it names: while a backgrounded subagent runs,
							"talked to Maja" carries the sweep alone instead of lending it to
							the settled commands sharing its sentence.
						-->
						<!--
							One element whether or not its clause is live. Branching to a plain
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
						<span class="min-w-0 flex-1 truncate text-inherit tabular-nums">
							{#each clauses as clause (clause.category)}
								<span
									class="trace-clause" data-entering={clause.entering ? "true" : undefined}
									><ShimmerText
										active={clause.live}
										class="overflow-hidden whitespace-nowrap text-inherit"
										>{#each clause.parts as part, part_index (part_index)}{#if part.type === "count"}{@render clause_count(
													part,
												)}{:else}{part.text}{/if}{/each}</ShimmerText></span
							>
							{/each}
						</span>
						<!-- The chevron belongs to the label: gap-1 from it, like the work-session header. -->
						<span class="-ml-1 flex shrink-0 origin-center transition-transform duration-(--acc-chevron) ease-(--acc-ease) group-data-[open=true]/trace-acc:rotate-90">
							<ChevronRight class="size-3.5" aria-hidden="true" />
						</span>
					</button>

					<div class="t-acc-panel">
						<div class="t-acc-panel-inner pt-1">
							<div class="relative flex flex-col gap-1 pl-6">
								<div
									aria-hidden="true"
									class="pointer-events-none absolute inset-y-0 left-0 w-4 after:absolute after:inset-y-0 after:left-1/2 after:w-[2px] after:-translate-x-1/2 after:bg-border/60"
								></div>
							{#each segment.items as activity (activity.id)}
								{@const activity_category = GetConversationActivityCategory(activity.kind)}
								<div class="w-full min-w-0 py-0.5 text-base text-muted-foreground">
									<span class="flex min-w-0 flex-row items-center gap-2">
										<span class="shrink-0 text-foreground">
											{GetConversationActivityCategoryLabel(activity_category)}
										</span>
										{#if activity.kind === "terminal_activity" && activity.detail !== undefined}
											<!-- The monospace face already says "shell"; a prompt glyph is noise. -->
											<span class="trace-command-label min-w-0 flex-1 font-mono text-sm text-muted-foreground">
												{PresentShellCommand(activity.detail)}
											</span>
										{:else}
											<!-- A row without its own detail still reads as normalized work, not a raw provider label. -->
											<span class="min-w-0 truncate text-muted-foreground">
												{activity.detail ??
													GetConversationActivityPresentation(activity).label}
											</span>
										{/if}
									</span>
								</div>
							{/each}
							</div>
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
					class="t-acc group/trace-acc flex flex-col"
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
						<span class="-ml-1 flex origin-center transition-transform duration-(--acc-chevron) ease-(--acc-ease) group-data-[open=true]/trace-acc:rotate-90">
							<ChevronRight class="size-3.5" aria-hidden="true" />
						</span>
					</button>

					<div class="t-acc-panel">
						<div class="t-acc-panel-inner flex flex-col gap-1 pt-1">
							{#each segment.items as diagnostic (diagnostic.id)}
								{#if diagnostic.error !== undefined}
									<!--
										A failure in Artisan custody explains itself as a card: the
										catalog names it, the code anchors it, and the provider's
										own words become the evidence line instead of the headline.
									-->
									<ConversationErrorCard
										error={diagnostic.error}
									/>
								{:else}
									<!--
										The header icon already names the tier for the whole group;
										rows indent under its label instead of repeating it down
										the list.
									-->
									<div class={`min-w-0 py-0.5 pl-6 text-sm ${presentation.row}`}>
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
