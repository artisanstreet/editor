<script lang="ts" effect>
	import { page } from "$app/state";
	import type { ProjectRepository, ThreadListItem } from "@artisan/protocol";
	import { ArtisanClient } from "@artisan/transport/client";
	import ArrowBarToDown from "@tabler/icons-svelte/icons/arrow-bar-to-down";
	import Folder from "@tabler/icons-svelte/icons/folder";
	import { Effect, Option, Stream } from "effect";
	import * as ContextMenu from "$lib/components/ui/context-menu";
	import {
		BrowserReaderAttention,
		reader_can_acknowledge_root_conversation,
	} from "$lib/browser/reader-attention";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { EngineMarkClass, UsageSlicePresentationFor } from "$lib/engine/presentation";
	import {
		ThreadOrchestrationRoster,
		type ThreadAgentInspection,
	} from "$lib/orchestration/service";
	import {
		FormatRecentThreadTime,
		PinnedThreads,
		ResolveThreadRoute,
		SettledThreads,
		SortRecentThreads,
		ThreadFailed,
		ThreadHasActiveWork,
		ThreadIsAwaitingAnswer,
		ThreadNeedsAttention,
		ThreadRailTimeGroups,
		ThreadReaderActivityAt,
		ThreadSettled,
		ThreadRoutePathFor,
	} from "$lib/root/thread-navigation";
	import {
		AdvanceThreadReadTracking,
		type ThreadReadTrackingState,
	} from "$lib/root/thread-read-tracker";
	import { thread_display_title, thread_title_mode } from "$lib/threads/title";
	import { RepositoryQualifiedLabel } from "$lib/vcs/labels";
	import { RepositoryMarkClass, RepositoryMarkFor } from "$lib/vcs/presentation";
	import { ThreadOpenController } from "$lib/thread-interaction/thread-open-controller";
	import { ProjectRepositoryController } from "$lib/workspace/project-repository-controller";
	import InlineCodeText from "$lib/components/inline-code-text.svelte";
	import ActiveThreadLight from "./active-thread-light.svelte";
	import DropdownHoverSurface from "./dropdown-hover-surface.svelte";
	import ShaderGlassSurface from "./shader-glass-surface.svelte";

	let {
		header_inset = false,
		suppressed = false,
		threads,
	}: {
		/**
		 * Whether a workspace header occupies the card's own top row. The web shell
		 * puts it there; the desktop shell titles from the window frame instead,
		 * which leaves the card's top edge free. The pinned group starts below it
		 * either way, and only the surface that renders the header knows which.
		 */
		header_inset?: boolean;
		/**
		 * Set while another surface owns the proximity band: a hover reveal must
		 * stand down underneath an overlapping menu or full-screen viewer.
		 */
		suppressed?: boolean;
		/** The live thread list, owned by the layout. */
		threads: ReadonlyArray<ThreadListItem>;
	} = $props();

	/** Captured on mount so relative times never sit stale on screen. */
	let now_ms = $state(Date.now());
	let near = $state(false);
	let zone_element = $state<HTMLDivElement>();
	/** Last pointer position, for reveal decisions that arrive without a pointer event. */
	let last_pointer_x = 0;
	let last_pointer_y = 0;
	const open = $derived(near && !suppressed);

	const active_route_id = $derived(page.params.thread);
	const client = yield* ArtisanClient;

	/**
	 * The thread on screen, if the reader is on one. Resolved the same way the
	 * rows link, so a historical `thread_` identity counts as open too.
	 */
	const open_thread = $derived(
		active_route_id === undefined
			? undefined
			: Option.getOrUndefined(ResolveThreadRoute(threads, active_route_id)),
	);
	/** Exact durable identity wins even when the route uses its historical bare alias. */
	const active_thread_id = $derived(open_thread?.thread_id);
	const reader_attention = yield* BrowserReaderAttention;
	let reader_is_watching = $state(yield* reader_attention.Current);
	const ApplyReaderAttention = (watching: boolean) =>
		Effect.gen(function* () {
			reader_is_watching = watching;
		});
	yield* reader_attention.Changes.pipe(
		Stream.runForEach(ApplyReaderAttention),
		Effect.forkScoped,
	);
	const orchestration = yield* ThreadOrchestrationRoster;
	let agent_inspection = $state.raw<ThreadAgentInspection | undefined>(
		yield* orchestration.CurrentInspection,
	);
	const inspecting_agent = $derived(agent_inspection !== undefined);
	const ApplyAgentInspection = (next: ThreadAgentInspection | undefined) =>
		Effect.gen(function* () {
			agent_inspection = next;
		});
	yield* orchestration.InspectionChanges.pipe(
		Stream.runForEach(ApplyAgentInspection),
		Effect.forkScoped,
	);
	const Settle = (thread: ThreadListItem) =>
		Effect.gen(function* () {
			/** An explicit acknowledgement is valid even while another participant is open. */
			if (ThreadHasActiveWork(thread) || ThreadSettled(thread)) return;
			yield* client.Command({
				payload: {
					reader_activity_at: ThreadReaderActivityAt(thread),
					type: "thread.attention.acknowledge",
				},
				thread_id: thread.thread_id,
			});
		});
	let read_tracking = $state.raw<ThreadReadTrackingState>({});
	/**
	 * Attention is acknowledged only across a real thread boundary. Re-entering
	 * or navigating to the same route keeps the indicator intact, while the
	 * observed stamp follows root activity the reader actually had on screen.
	 */
	const AcknowledgeDepartedThread = (
		next_route_id: string | undefined,
		next_thread: ThreadListItem | undefined,
		watching: boolean,
		inspecting_agent: boolean,
	) =>
		Effect.gen(function* () {
			const transition = AdvanceThreadReadTracking(read_tracking, {
				root_visible: reader_can_acknowledge_root_conversation(
					watching,
					inspecting_agent,
				),
				route_id: next_route_id,
				thread: next_thread,
			});
			read_tracking = transition.state;
			if (transition.acknowledgement === undefined) return;
			/**
			 * Publishing the next state before admission makes a rerun observe the
			 * new route, so the one transition cannot enqueue a duplicate command.
			 */
			const acknowledgement = client.Command({
				payload: {
					reader_activity_at: transition.acknowledgement.reader_activity_at,
					type: "thread.attention.acknowledge",
				},
				thread_id: transition.acknowledgement.thread_id,
			});
			/** The persistent rail scope lets this durable command outlive the reactive run. */
			yield* acknowledgement.pipe(
				Effect.forkScoped,
			);
		});

	/**
	 * Unread floats to the top of both lists, freshest first inside each group —
	 * the shape the model picker uses to lift favourites above their engine. The
	 * rows carry a transform transition so a thread that settles while the list
	 * is on screen slides up rather than teleporting.
	 */
	const recent_threads = $derived(SettledThreads(threads));

	const recent_thread_groups = $derived(ThreadRailTimeGroups(recent_threads, now_ms));
	const working_threads = $derived(PinnedThreads(threads));
	const recent_light_layout = $derived(
		recent_thread_groups
			.map(
				(group) =>
					`${group.id}:${group.label ?? ""}:${group.threads.map((thread) => thread.thread_id).join(",")}`,
			)
			.join("\n"),
	);
	const working_light_layout = $derived(
		working_threads.map((thread) => thread.thread_id).join("\n"),
	);
	let recent_light_surface = $state<HTMLElement | null>(null);
	let working_light_surface = $state<HTMLElement | null>(null);
	/**
	 * The card grows with its rows until it would take the whole rail, and only
	 * then scrolls. Ten is the point where the group stops reading as "what is
	 * happening" and starts reading as a second list.
	 */
	const VisibleWorkingRows = 10;
	/**
	 * The cap is a row count, not a length, and every row here is the same two
	 * lines — so the height of one is the height of all of them divided by how
	 * many there are. Measured rather than written down, because a hardcoded rem
	 * silently becomes 9.4 rows the day the row's type changes, and a card that
	 * cuts its tenth row in half is worse than one that never capped.
	 */
	let working_rows_height = $state(0);
	const working_row_height = $derived(
		working_threads.length > 0 ? working_rows_height / working_threads.length : 0,
	);
	const working_scrolls = $derived(
		working_row_height > 0 && working_threads.length > VisibleWorkingRows,
	);
	/**
	 * The context-menu trigger hands its props untyped, and one of them is the
	 * pointer-move that cancels a touch long-press. The row owns that event too —
	 * it is how the hover pill follows the pointer — so both run rather than the
	 * row's handler quietly replacing the trigger's.
	 */
	const RowPointerMove =
		(trigger_props: Record<string, unknown>, move_hover: (event: Event) => void) =>
		(event: PointerEvent) => {
			const cancel_long_press = trigger_props.onpointermove;
			if (typeof cancel_long_press === "function") cancel_long_press(event);
			move_hover(event);
		};

	/**
	 * One hover card serves both rail lists. It reveals after a short dwell and
	 * then travels: pointing at another row slides the same card vertically the
	 * way the hover pill slides, instead of dismounting one card and mounting
	 * another. The handlers are direct (synchronous) because they read row
	 * geometry off `currentTarget`, which only exists while the event is
	 * dispatching — the same constraint the pill's `move_hover` carries.
	 */
	let card_thread = $state.raw<ThreadListItem | undefined>(undefined);
	let card_frame = $state<HTMLElement>();
	let card_engaged = $state(false);
	let card_travelled = $state(false);
	let card_y = $state(0);

	/**
	 * The same retained repository projection the workspace header paints from,
	 * so the card names a linked project exactly the way the header does —
	 * remote mark and qualified label — and a local one by its folder.
	 * Inspection is lazy: only the project the card is currently naming.
	 */
	const repository_controller = yield* ProjectRepositoryController;
	let card_repositories = $state.raw<ReadonlyMap<string, ProjectRepository | undefined>>(
		yield* repository_controller.Current,
	);
	const ApplyCardRepositories = (
		next: ReadonlyMap<string, ProjectRepository | undefined>,
	) =>
		Effect.sync(() => {
			card_repositories = next;
		});
	yield* repository_controller.Changes.pipe(
		Stream.runForEach(ApplyCardRepositories),
		Effect.forkScoped,
	);
	/** Rerun per hovered project; a superseded inspection may be abandoned. */
	const RefreshCardRepository = (project_id: string | undefined) =>
		project_id === undefined
			? Effect.void
			: repository_controller.Refresh(project_id).pipe(Effect.forkScoped);
	yield* RefreshCardRepository(card_thread?.primary_project?.project_id);

	/**
	 * Pointing at a row is the earliest honest signal that it is about to be
	 * opened, and the aggregate behind it takes a round trip Forge can start now
	 * instead of after the click. By the time the reader has crossed the row and
	 * pressed it the snapshot is usually resident, which is the difference
	 * between a route that paints and a route that shows a skeleton first.
	 *
	 * Same shape as the repository inspection above — per hovered row, forked,
	 * abandonable — and the controller's single-flight is what makes sweeping
	 * the list cost one request per thread rather than one per pointer event.
	 */
	const thread_opens_prefetch = yield* ThreadOpenController;
	const PrefetchThread = (thread_id: string | undefined) =>
		thread_id === undefined ? Effect.void : thread_opens_prefetch.Prefetch(thread_id);
	yield* PrefetchThread(card_thread?.thread_id);

	const place_card = (row: HTMLElement) => {
		if (zone_element === undefined) return;
		const zone = zone_element.getBoundingClientRect();
		const rect = row.getBoundingClientRect();
		const height = card_frame?.offsetHeight ?? 0;
		card_y = Math.max(8, Math.min(rect.top - zone.top, zone.height - height - 8));
	};

	const hide_card = () => {
		card_engaged = false;
		card_travelled = false;
	};

	/**
	 * The reveal dwell is purely a CSS transition delay: engaging any row
	 * starts the fade-in after that delay, retargets while the card is still
	 * transparent slide it silently, and once it has faded in each retarget
	 * reads as an animated tick down or up the list.
	 */
	const RowHover =
		(move_hover: (event: Event) => void, thread: ThreadListItem) => (event: Event) => {
			move_hover(event);
			const row = event.currentTarget;
			if (!(row instanceof HTMLElement)) return;
			card_travelled = card_engaged;
			card_thread = thread;
			place_card(row);
			card_engaged = true;
		};

	const Reveal = () =>
		Effect.gen(function* () {
			if (suppressed) return;
			if (!near) now_ms = Date.now();
			near = true;
		});
	const Conceal = () =>
		Effect.gen(function* () {
			near = false;
			hide_card();
		});
	/**
	 * Opening a thread moves focus off the zone — the router resets focus after
	 * every navigation — while the reader's pointer is usually still resting in
	 * the band, mid-list, about to open the next thread. Focus loss therefore
	 * only conceals once the pointer has genuinely left; while it remains
	 * inside, proximity keeps owning the reveal exactly as if it had moved.
	 */
	const ConcealOnFocusOut = (event: FocusEvent) =>
		Effect.gen(function* () {
			const inside = yield* RunBrowserDom(() => {
				const next = event.relatedTarget;
				if (next instanceof Node && zone_element?.contains(next)) return true;
				const zone = zone_element?.getBoundingClientRect();
				return (
					zone !== undefined &&
					zone.width > 0 &&
					last_pointer_x >= zone.left &&
					last_pointer_x <= zone.right &&
					last_pointer_y >= zone.top &&
					last_pointer_y <= zone.bottom
				);
			}).pipe(
				Effect.catch(() =>
					Effect.gen(function* () {
						return false;
					}),
				),
			);
			if (!inside) yield* Conceal();
		});
	/**
	 * Menu state can change from the keyboard without another pointer event. Clear
	 * remembered proximity reactively so closing that menu cannot replay a hover
	 * that happened before it opened.
	 */
	const ReconcileSuppression = (is_suppressed: boolean, proximity_active: boolean) =>
		Effect.gen(function* () {
			if (is_suppressed) hide_card();
			if (is_suppressed && proximity_active) near = false;
		});
	yield* ReconcileSuppression(suppressed, near);
	/**
	 * The closed rail cannot own pointer events without blocking the surface under
	 * it. Read proximity from the window against the real margin bounds instead;
	 * once revealed, the list itself becomes interactive inside that same band.
	 */
	const TrackPointer = (event: PointerEvent) =>
		Effect.gen(function* () {
			last_pointer_x = event.clientX;
			last_pointer_y = event.clientY;
			if (zone_element === undefined) return;
			/**
			 * Drop proximity instead of freezing it so closing the overlay cannot
			 * reveal a list banked by the pointer resting on that overlay.
			 */
			if (suppressed) {
				yield* Conceal();
				return;
			}
			const bounds = yield* RunBrowserDom(() => ({
				/** A visible card widens the band: reading it must not conceal it. */
				card: card_engaged ? card_frame?.getBoundingClientRect() : undefined,
				zone: zone_element?.getBoundingClientRect(),
			}));
			/**
			 * The card sits a gap to the right of the band, and that gap belonged
			 * to neither rectangle — so the pointer travelling toward the card left
			 * the band mid-trip and concealed the very thing it was reaching for.
			 * Extending the card's hit area back to the band's own edge makes the
			 * crossing continuous, which is what the card needs to be readable at
			 * all. Geometry rather than a delayed dismissal: a grace long enough
			 * to survive the trip also strands the card on screen well after the
			 * reader has gone somewhere else.
			 */
			const reachable_card =
				bounds.card === undefined || bounds.zone === undefined
					? bounds.card
					: {
							bottom: bounds.card.bottom,
							left: bounds.zone.right,
							right: bounds.card.right,
							top: bounds.card.top,
							width: bounds.card.right - bounds.zone.right,
						};
			const within = (
				rect:
					| {
							readonly bottom: number;
							readonly left: number;
							readonly right: number;
							readonly top: number;
							readonly width: number;
					  }
					| undefined,
			) =>
				rect !== undefined &&
				rect.width > 0 &&
				event.clientX >= rect.left &&
				event.clientX <= rect.right &&
				event.clientY >= rect.top &&
				event.clientY <= rect.bottom;
			if (within(bounds.zone) || within(reachable_card)) yield* Reveal();
			else if (near) yield* Conceal();
		});

	/** Same-route updates only refresh the observed stamp; only departure writes it. */
	yield* AcknowledgeDepartedThread(
		active_route_id,
		open_thread,
		reader_is_watching,
		inspecting_agent,
	);

</script>

<!--
	Any scroll moves rows under a parked card; hiding beats chasing stale geometry.

	The wheel is listened for separately because the card now takes pointer
	events: a wheel over it has no scrollable ancestor of its own, so the gesture
	would land on the card and move nothing at all while the transcript sat still
	underneath. Dismissing on the first tick hands the rest of the gesture — and
	its momentum — straight back to the transcript.
-->
<svelte:window
	onpointermove={yield* TrackPointer(event)}
	onscrollcapture={hide_card}
	onwheelcapture={hide_card}
/>

{#snippet thread_rows(move_hover: (event: Event) => void)}
	{#each recent_thread_groups as group, group_index (group.id)}
		<section class="flex flex-col">
			{#if group.label !== undefined}
				<!--
					Later headers take their room above from the group gap. A header that
					opens the card has only the glass gutter over it, so it carries the
					rows' own top inset instead of pressing bare text against the edge.
				-->
				<h2
					class={`px-2 pb-1 text-xs font-medium text-muted-foreground ${group_index === 0 ? "pt-2" : ""}`}
				>
					{group.label}
				</h2>
			{/if}
			{#each group.threads as thread (thread.thread_id)}
				{@const is_active = thread.thread_id === active_thread_id}
				{@const thread_mark = UsageSlicePresentationFor(thread.engine_id, thread.model_id).mark}
				{@const ThreadMark = thread_mark.icon}
				<a
					href={ThreadRoutePathFor(thread)}
					class={`relative flex min-w-0 items-center gap-2 rounded-lg px-2 py-2 text-sm font-medium outline-none [transition-property:color,transform] duration-(--duration-fast) ease-(--ease-smooth-out) will-change-transform focus-visible:text-foreground-extra motion-reduce:transition-none ${is_active ? "text-foreground" : "text-muted-foreground hover:text-foreground-extra"}`}
					aria-current={is_active ? "page" : undefined}
					onpointerenter={RowHover(move_hover, thread)}
					onpointermove={move_hover}
					onfocusin={RowHover(move_hover, thread)}
				>
					<!-- The rail names the same thing the list does: what the thread runs on. -->
					<ThreadMark class={EngineMarkClass(thread_mark, "size-4 shrink-0")} />
					<span class="min-w-0 flex-1 truncate"
						>{thread_display_title(thread, $thread_title_mode)}</span
					>
					<span class="shrink-0 whitespace-nowrap text-xs text-muted-foreground @max-[13rem]:hidden">
						{FormatRecentThreadTime(thread.last_activity_at, now_ms)}
					</span>
				</a>
			{/each}
		</section>
	{/each}
{/snippet}

<!--
	The margin hosts active work and reveals complete history on proximity on
	every thread surface, including `/`. Panel reveal carries the list in and out;
	the closed rail remains pointer-inert so it never blocks the surface beneath.

	The band is the margin, not a fixed width, and the margin is the whole left
	side: the reading column sits right of centre precisely so this band gets
	the surplus rather than splitting it with dead space, keeping only the
	gutter on the far side. It is spelled from the same tokens the column
	offsets itself with, so a looser column narrows the band instead of
	overrunning it, and it stops a gap short of the column rather than reaching
	under it — the composer card's edge is the column's edge, and it was sitting
	over the rows. The band is a container so the paddings and the timestamp can
	answer to the room it actually got rather than to the window.

	Inset by the card's own 0.25rem padding on both sides, because that is the
	box the column measures its margin from; the band has to be the same margin,
	not one 0.25rem wider on each side.

	The list lives in the margin or it does not appear: nothing floats over the
	transcript. Below 9rem of band there is no legible row to draw, and the
	shell answers a window that narrow by dropping the inspector column, which
	hands this margin the room it needs. Narrower still than that, the command
	menu is where threads are reached.
-->
<div
	bind:this={zone_element}
	class="@container pointer-events-none absolute inset-y-0 left-1 z-30 w-[clamp(0rem,calc(100%_-_0.5rem_-_var(--prose-width)_-_var(--prose-gutter)_-_var(--prose-rail-gap)),calc(var(--prose-rail-margin)_-_var(--prose-rail-gap)))]"
	role="presentation"
	onfocusin={yield* Reveal()}
	onfocusout={yield* ConcealOnFocusOut(event)}
><!--
		The band carries a stacking context, so nothing inside it can outrank the
		composer card by raising its own `z-index` — the whole subtree is ordered
		by this one value. It therefore sits above the composer's own layer, which
		is what lets the hover card overlay the composer instead of sliding under
		it. Overlapping menus and viewers still win: they take the band down
		through `suppressed` rather than by out-stacking it.
	-->
	<div
		class={`absolute inset-0 hidden min-h-0 flex-col pr-2 pl-6 @min-[9rem]:flex ${working_threads.length > 0 ? "gap-4" : ""} ${header_inset ? "pt-16 pb-6" : "py-6"}`}
	>
		<!--
			The working group is as tall as the threads that are in it, up to the row
			cap. The sibling list therefore starts after a real gap instead of
			painting under an independently positioned card.
		-->
		{#if working_threads.length > 0}
			<!--
				The model picker's shape exactly: this wrapper only positions, and the
				card keeps its own edge. Neither of them scrolls or is masked — the cap
				belongs inside the card's padding, because masking the wrapper would eat
				the glass edge and take the shadow with it. The list below is what
				yields the room this group takes.
			-->
			<div
				class="pointer-events-auto min-h-0 w-full max-w-(--inspector-width) shrink-0"
				aria-label="Working"
			>
				<ShaderGlassSurface class="t-resize t-resize-auto min-h-0 max-h-full shrink rounded-xl">
					<div class="min-w-0 p-1">
						<!--
							The cap lives inside the card's padding, so the glass edge and its
							shadow stay untouched and the fade only ever crosses rows. Both the
							height and the fade are conditional: below the cap there is nothing
							past an edge, and a mask that dims a row nothing is hidden behind
							reads as a rendering fault.
						-->
						<div
							bind:this={working_light_surface}
							class={working_scrolls ? "docs-scroll-fade overflow-x-hidden overflow-y-auto" : ""}
							class:relative={true}
							style={working_scrolls
								? `max-height:${working_row_height * VisibleWorkingRows}px`
								: ""}
						>
							<ActiveThreadLight
								{active_thread_id}
								layout_key={working_light_layout}
								surface={working_light_surface}
							/>
							<!--
								The model picker's hover: one flat pill slides between rows
								rather than each row fading its own highlight in.
							-->
							<div class="relative z-2">
								<DropdownHoverSurface class="[--docs-sidebar-hover-radius:var(--radius-lg)]">
								{#snippet children({ move_hover })}
									<div class="flex flex-col" bind:clientHeight={working_rows_height}>
										{#each working_threads as thread (thread.thread_id)}
											{@const is_active = thread.thread_id === active_thread_id}
											{@const thread_mark = UsageSlicePresentationFor(thread.engine_id, thread.model_id).mark}
											{@const ThreadMark = thread_mark.icon}
											<!-- Only an inactive unread outcome asks for attention. -->
											{@const needs_attention = ThreadNeedsAttention(thread)}
											{@const awaiting_answer = ThreadIsAwaitingAnswer(thread)}
											<!-- The row stays the link it was; the trigger renders through it. -->
											<ContextMenu.Root>
												<ContextMenu.Trigger
													class="relative flex min-w-0 items-center gap-2 rounded-lg px-2 py-2 text-sm outline-none"
												>
													{#snippet child({ props })}
														<a
																	{...props}
																	href={ThreadRoutePathFor(thread)}
																	aria-current={is_active ? "page" : undefined}
															onpointerenter={RowHover(move_hover, thread)}
															onpointermove={RowPointerMove(props, move_hover)}
															onfocusin={RowHover(move_hover, thread)}
														>
																<ThreadMark class={EngineMarkClass(thread_mark, "size-4 shrink-0")} />
															<span class="flex min-w-0 flex-1 flex-col">
																<!-- Ellipsis, not a fade: the two rail cards are one surface. -->
																<span class="min-w-0 truncate text-foreground"
																	>{thread_display_title(thread, $thread_title_mode)}</span
																>
																	<span class="truncate text-xs text-muted-foreground">
																		{thread.primary_project?.display_name ?? "No project"}
																	</span>
															</span>
															<!-- Same cell the list rows give it: end of the row, centred on it. -->
															<span
																class={`state-dot size-1.5 shrink-0 rounded-full ${awaiting_answer ? "shadow-[0_0_8px_var(--color-purple-400)]" : ""}`}
																style={awaiting_answer
																	? "--state-dot-tone: var(--color-purple-400)"
																	: needs_attention
																	? ThreadFailed(thread)
																		? "--state-dot-tone: var(--destructive)"
																		: "--state-dot-tone: var(--unread)"
																	: ""}
																aria-label={awaiting_answer
																	? "Question awaiting answer"
																	: needs_attention
																	? (ThreadFailed(thread) ? "Unread failure" : "Unread")
																	: undefined}
															></span>
														</a>
													{/snippet}
												</ContextMenu.Trigger>
												{#if !ThreadHasActiveWork(thread)}
													<ContextMenu.Content class="w-48">
														<ContextMenu.Item onclick={yield* Settle(thread)}>
															<ArrowBarToDown />
															Settle
														</ContextMenu.Item>
													</ContextMenu.Content>
												{/if}
											</ContextMenu.Root>
										{/each}
									</div>
								{/snippet}
								</DropdownHoverSurface>
							</div>
						</div>
					</div>
				</ShaderGlassSurface>
			</div>
		{/if}

		<div
			class="t-panel-slide-x pointer-events-auto flex min-h-0 flex-1 flex-col gap-3"
			style="--panel-translate-x: -16px"
			data-open={open}
			inert={!open}
			aria-label="All threads"
		>
			<!--
				The same card the inspector's environment panel is, on the other
				margin of the same column: one glass surface, one width token, and the
				pill that slides between its rows. It was a bare list of text sitting
				directly on the background — the only surface on either side of the
				transcript that was not a card — which is what made a long title read
				as running off the edge of the window rather than out of a card.
			-->
			<div class="flex min-h-0 flex-1 flex-col">
				<ShaderGlassSurface
					content_column
					class="radius-surface min-h-0 max-h-full w-full max-w-(--inspector-width) shrink [--radius-gap:var(--spacing)] [--radius-surface:var(--radius-xl)]"
				>
					<!--
						The card is a flex column down to the scroll box, and every step of
						that chain carries `min-h-0`. Without it a flex item refuses to
						shrink below its content, so the list simply grew past the card and
						the glass clipped whatever did not fit — a scroll box that had
						nothing to scroll because it was never the thing being constrained.
					-->
					<div class="flex min-h-0 min-w-0 flex-col p-1">
						<div
							bind:this={recent_light_surface}
							class="docs-scroll-fade relative flex min-h-0 flex-col gap-3 overflow-x-hidden overflow-y-auto"
						>
							<ActiveThreadLight
								{active_thread_id}
								layout_key={recent_light_layout}
								surface={recent_light_surface}
							/>
							<div class="relative z-2">
								<DropdownHoverSurface
									class="[--docs-sidebar-hover-radius:var(--radius-lg)]"
								>
									{#snippet children({ move_hover })}
										<div class="flex flex-col gap-3">
											{@render thread_rows(move_hover)}
										</div>
									{/snippet}
								</DropdownHoverSurface>
							</div>
						</div>
					</div>
				</ShaderGlassSurface>
			</div>
		</div>
	</div>

	<!--
		The rail's hover card: identity in full, then what the thread is doing,
		then where — never the configuration table a click already answers. One
		instance travels between rows on the pill's own motion tokens; it takes
		no pointer events, so it can overlay the transcript without stealing it.
	-->
	{#if card_thread !== undefined}
		{@const card_mark = UsageSlicePresentationFor(card_thread.engine_id, card_thread.model_id).mark}
		{@const CardMark = card_mark.icon}
		{@const card_awaiting = ThreadIsAwaitingAnswer(card_thread)}
		{@const card_failed = ThreadFailed(card_thread)}
		{@const card_working = ThreadHasActiveWork(card_thread)}
		<!-- The header's own project grammar: remote mark for a linked repository, folder for local. -->
		{@const card_repository =
			card_thread.primary_project === undefined
				? undefined
				: card_repositories.get(card_thread.primary_project.project_id)}
		{@const card_remote =
			card_repository?.state === "repository"
				? card_repository.remotes.find(
						(candidate) => candidate.name === card_repository.default_remote,
					)
				: undefined}
		<!--
			Half the rail's own gap: the card reads as attached to the row it names,
			and the pointer crosses to it over a distance short enough to feel like
			one surface rather than a jump. It takes pointer events only once
			engaged, so a reader can rest on it and select its text, while a card
			that has not been reached still lets the transcript beneath it through.
		-->
		<div
			bind:this={card_frame}
			aria-hidden="true"
			data-travelled={card_travelled}
			class={`absolute top-0 left-full z-40 ml-0.5 w-64 will-change-transform data-[travelled=true]:transition-transform data-[travelled=true]:duration-(--duration-fast) data-[travelled=true]:ease-(--ease-smooth-out) motion-reduce:transition-none ${card_engaged ? "pointer-events-auto" : "pointer-events-none"}`}
			style={`transform: translate3d(0, ${card_y}px, 0)`}
		>
			<!--
				The tooltip entrance: the dwell delays only the reveal, so the travel
				above stays undelayed and concealing snaps. The card grows in from the
				row's own edge rather than only fading, which is what makes it read as
				coming from the row it names.
			-->
			<div
				data-shown={card_engaged}
				class="t-tt origin-left [--tt-delay:450ms]"
			>
			<ShaderGlassSurface class="rounded-xl">
				<div class="relative flex min-w-0 flex-col gap-1.5 p-3">
					<!--
						The card opens the thread it is describing, so pointing at a row and
						reading its card ends the same way pointing at the row does. It
						covers the card rather than wrapping it, because the repository
						below is a link of its own and an anchor cannot hold another one —
						the destination that owns the whole surface is the one that has to
						give up the wrapper.

						Deliberately unfocusable: the row directly behind it is the same
						destination and is already in the tab order, so a second stop there
						would make every thread cost two presses and announce itself twice.
						That is also what keeps the frame's `aria-hidden` honest — nothing
						inside it can take focus.
					-->
					<a
						href={ThreadRoutePathFor(card_thread)}
						tabindex="-1"
						onclick={hide_card}
						class="absolute inset-0 rounded-xl"
					></a>
					<div class="flex items-start gap-2">
						<CardMark class={EngineMarkClass(card_mark, "mt-0.5 size-4 shrink-0")} />
						<!-- Two lines at most: the card recovers a truncated title, not a paragraph. -->
						<span
							class="line-clamp-2 min-w-0 text-sm font-semibold text-pretty break-words text-foreground"
						>
							<InlineCodeText text={thread_display_title(card_thread, $thread_title_mode)} />
						</span>
					</div>
					{#if card_awaiting}
						<span class="text-xs" style="color: var(--color-purple-400)">
							Waiting for your answer
						</span>
					{:else if card_failed}
						<span class="text-xs text-destructive">Failed — needs attention</span>
					{:else if card_working}
						<span class="line-clamp-2 text-xs text-muted-foreground">
							<InlineCodeText text={card_thread.current_goal ?? "Working"} />
						</span>
					{:else if card_thread.last_assistant_message !== undefined}
						<!-- The model's own words, so its backticks read as the code face, not as marks. -->
						<span class="line-clamp-3 text-xs text-muted-foreground">
							<InlineCodeText text={card_thread.last_assistant_message} />
						</span>
					{/if}
					<span class="flex min-w-0 items-center justify-between gap-2 text-xs text-muted-foreground">
						<span class="flex min-w-0 items-center gap-1.5">
							{#if card_remote?.web_url !== undefined}
								{@const card_repo_mark = RepositoryMarkFor(card_remote.host)}
								{@const CardRepoIcon = card_repo_mark.icon}
								<CardRepoIcon
									class={RepositoryMarkClass(card_repo_mark, "size-3.5 shrink-0 translate-y-px")}
								/>
								<!--
									The header's tone and destination for the same fact: a project
									with a remote reads in the linked colour and opens that remote,
									a local one stays muted beside its folder. The mark keeps its
									host's brand, as it does up there.

									Positioned so it sits over the card's own stretched link.
									Without that, the surface that opens the thread would swallow
									the one click on it meant to go somewhere else.
								-->
								<a
									href={card_remote.web_url}
									target="_blank"
									rel="noreferrer"
									tabindex="-1"
									onclick={hide_card}
									class="relative z-1 min-w-0 truncate text-(--banner-info) underline-offset-2 hover:underline"
								>
									{RepositoryQualifiedLabel(card_remote.web_url)}
								</a>
							{:else}
								<Folder class="size-3.5 shrink-0 translate-y-px" />
								<span class="min-w-0 truncate">
									{card_thread.primary_project?.display_name ?? "No project"}
								</span>
							{/if}
						</span>
						<span class="shrink-0">
							{FormatRecentThreadTime(card_thread.last_activity_at, now_ms)}
						</span>
					</span>
				</div>
			</ShaderGlassSurface>
			</div>
		</div>
	{/if}
</div>
