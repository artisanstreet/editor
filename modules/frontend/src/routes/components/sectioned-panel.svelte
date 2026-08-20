<script lang="ts" effect>
	import { Effect, Stream } from "effect";
	import type { Snippet } from "svelte";
	import type { ThreadListItem } from "@artisan/protocol";
	import CodeIcon from "@tabler/icons-svelte/icons/code";
	import MessageCircle from "@tabler/icons-svelte/icons/message-circle";
	import MessagePlus from "@tabler/icons-svelte/icons/message-plus";
	import ShoppingBag from "@tabler/icons-svelte/icons/shopping-bag";

	import artisan_star from "$lib/assets/barekey/artisan-star.svg";
	import logo_gradient from "$lib/assets/barekey/logo-gradient.svg";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { RouteNavigation } from "$lib/browser/route-navigation";
	import { EditorRoutePath } from "$lib/editor/workspace-identity";
	import { ImageInspectionStore } from "$lib/images/inspection-store";
	import {
		PrepareNewThreadDraft,
		is_unmodified_primary_activation,
		new_thread_draft_key,
	} from "$lib/root/new-thread-draft";
	import { ThreadRoutePath, WorkspaceRoutePath } from "$lib/root/thread-navigation";
	import CommandMenu from "./command-menu.svelte";
	import DropdownHoverSurface from "./dropdown-hover-surface.svelte";
	import SidebarIdentity from "./sidebar-identity.svelte";
	import ThreadHoverRail from "./thread-hover-rail.svelte";

	/**
	 * Which workspace you are in is a mode, not a tab: a thread and a file are
	 * two ways of working in the same project rather than two destinations to
	 * pick between. Ordered so the control cycles rather than toggles, which is
	 * what lets a third surface join without changing the interaction.
	 */
	const surfaces = [
		{ icon: MessageCircle, id: "threads", label: "Threads" },
		{ icon: CodeIcon, id: "editor", label: "Editor" },
	] as const;

	const current_index = $derived(surface === "editor" ? 1 : 0);
	const next = $derived(
		surfaces[(current_index + 1) % surfaces.length] ?? surfaces[0],
	);

	let command_open = $state(false);
	/**
	 * The account menu opens upward across the transcript's left margin, which is
	 * exactly the band that hosts working threads. Held here because the two live
	 * on opposite sides of the layout and neither owns the other.
	 */
	let account_open = $state(false);

	let {
		header,
		primary,
		secondary,
		surface,
		thread_rail,
		thread_id,
		threads,
		threads_loaded,
		workspace_id,
	}: {
		/**
		 * The workspace identity band across the primary card's top. Web only:
		 * the bundled shell carries the same identity in its window frame, so the
		 * layout passes nothing there rather than naming the workspace twice.
		 */
		header?: Snippet;
		primary: Snippet;
		secondary?: Snippet;
		/** Which workspace surface is on screen, owned by the layout. */
		surface: "editor" | "threads";
		/**
		 * How the left margin carries the thread list on this route, decided by the
		 * layout: proximity-revealed on thread surfaces and absent elsewhere.
		 */
		thread_rail: "hidden" | "proximity";
		/** The durable thread shared by both workspace surfaces. */
		thread_id: string | undefined;
		/** The live thread list, owned by the layout and shared with the command menu. */
		threads: ReadonlyArray<ThreadListItem>;
		/**
		 * Whether the thread list has actually arrived. An empty array means two
		 * different things before and after it does, and the rail must not treat
		 * them alike.
		 */
		threads_loaded: boolean;
		/** The workspace the current route is inside, resolved by the layout. */
		workspace_id: string | undefined;
	} = $props();

	const workspace_open = $derived(workspace_id !== undefined && thread_id !== undefined);
	/**
	 * A new thread belongs to the project you are already in. Outside one there is
	 * no project to start it in, so the action is the picker — which is the same
	 * question asked one step earlier rather than a different destination.
	 */
	const new_thread_path = $derived(
		workspace_id === undefined ? "/" : WorkspaceRoutePath(workspace_id),
	);
	const new_thread_key = $derived(new_thread_draft_key(workspace_id));
	/**
	 * Whether the transcript's left margin is actually carrying the thread list.
	 * It decides both that the rail mounts and that the reading column shifts
	 * right to feed it, so the two can never disagree about which side is doing
	 * work.
	 */
	/**
	 * An unloaded list holds the rail open rather than closing it. Closing on an
	 * empty array collapsed the margin whenever the list had not arrived yet,
	 * which drew "we do not know your threads" exactly like "you have none" and
	 * took both groups down with it — the reachable case being a subscribe the
	 * backend never answered. Absence is now only ever claimed once it is known.
	 */
	const rail_open = $derived(thread_rail !== "hidden" && (!threads_loaded || threads.length > 0));
	const navigation = yield* RouteNavigation;
	const StartNewThread = (event: MouseEvent) =>
		Effect.gen(function* () {
			if (!is_unmodified_primary_activation(event)) return;
			yield* RunBrowserDom(() => event.preventDefault());
			/**
			 * A retained first message refuses the reset and keeps its recovery
			 * state — but the navigation is still the user's intent, and the new
			 * thread surface is where that retained message is explained and
			 * retried. Failing here instead made this action silently do nothing.
			 */
			yield* PrepareNewThreadDraft(new_thread_key).pipe(
				Effect.catchTag("DraftThreadLocked", () => Effect.void),
			);
			yield* navigation.Navigate(new_thread_path);
		});

	/** The rail must not creep in over a full-screen image. */
	const inspection = yield* ImageInspectionStore;
	let inspecting_image = $state(yield* inspection.Current);
	const ApplyInspection = (open: boolean) =>
		Effect.gen(function* () {
			inspecting_image = open;
		});
	yield* inspection.Changes.pipe(Stream.runForEach(ApplyInspection), Effect.forkScoped);

	const CycleSurface = () =>
		Effect.gen(function* () {
			if (workspace_id === undefined || thread_id === undefined) return;
			yield* navigation.Navigate(
				surface === "editor"
					? ThreadRoutePath(workspace_id, thread_id)
					: EditorRoutePath(workspace_id, thread_id),
			);
		});
</script>

<div class="flex h-full min-h-0 flex-row">
	<!--
		The rail is the entire sidebar: no expanded panel behind it, so nothing
		here toggles. Thread navigation lives in the command menu instead.

		It renders at every width. Hiding it below a breakpoint is left over from
		when this was a real collapsing sidebar with a drawer to fall back on;
		there is no drawer now, so hiding it took the home link, the command menu,
		and the account with it and left the app with no navigation at all.
	-->
	<div class="relative block h-full w-14 shrink-0">
		<div class="absolute inset-x-0 top-2 flex flex-col items-center">
			<!--
				One flat hover surface spans both housings, so the pill is shared:
				it slides from the always-there controls down into the surface
				cycle and back, crossing the gap, instead of each housing fading
				its own highlight in. Flat layering is what lets the static card
				backgrounds paint under the pill while the relative controls
				paint above it.
			-->
			<DropdownHoverSurface flat class="[--docs-sidebar-hover-radius:9999px]">
				{#snippet children({ move_hover })}
					<div class="flex flex-col items-center gap-2">
						<!--
							One pill for what is always there: the brand mark, the new-thread
							action, and the marketplace belong to this edge unconditionally,
							so they share a housing rather than each floating on their own.
						-->
						<div class="w-10 rounded-full bg-surface-125 py-1 card dark:bg-surface-900">
							<div class="flex w-full flex-col items-center gap-1">
								<a
									href="/"
									aria-label="Artisan Editor home"
									class="group/artisan-logo relative isolate flex size-8 items-center justify-center overflow-hidden rounded-full outline-none card-plastic focus-visible:ring-2 focus-visible:ring-ring/50"
									style={`--artisan-logo-gradient: url(${logo_gradient});`}
									onpointerenter={move_hover}
									onpointermove={move_hover}
									onfocusin={move_hover}
								>
									<span
										aria-hidden="true"
										class="absolute inset-0 -z-10 bg-cover bg-center opacity-0 transition-opacity duration-(--duration-quick) ease-in-out group-hover/artisan-logo:opacity-100 group-focus-visible/artisan-logo:opacity-100 motion-reduce:transition-none"
										style="background-image: var(--artisan-logo-gradient);"
									></span>
									<img alt="" src={artisan_star} class="size-5 shrink-0" />
								</a>

								<!--
								Two hairlines, not one: the background-coloured line above the
								border reads as a cut through the rail rather than a line drawn
								on it. Height is literal so the pair stays 1px + 1px whatever
								the root font size is.
							-->
							<span
								aria-hidden="true"
								class="h-[2px] w-full shrink-0 border-t border-background bg-border"
							></span>

								<a
									href={new_thread_path}
									aria-label="New thread"
									class="group/new-thread relative flex size-8 items-center justify-center rounded-full outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
									onclick={yield* StartNewThread(event)}
									onpointerenter={move_hover}
									onpointermove={move_hover}
									onfocusin={move_hover}
								>
									<MessagePlus
										class="size-4 text-muted-foreground transition-colors duration-(--duration-fast) ease-in-out group-hover/new-thread:text-foreground motion-reduce:transition-none"
									/>
								</a>

								<button
									type="button"
									aria-label="Marketplace"
									class="group/marketplace relative flex size-8 items-center justify-center rounded-full outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
									onpointerenter={move_hover}
									onpointermove={move_hover}
									onfocusin={move_hover}
								>
									<ShoppingBag
										class="size-4 text-muted-foreground transition-colors duration-(--duration-fast) ease-in-out group-hover/marketplace:text-foreground motion-reduce:transition-none"
									/>
								</button>
							</div>
						</div>

						<!--
							The surface cycle only exists while a workspace is open for the
							editor to use, so it lives in its own housing rather than the
							always-there pill. It stays mounted and reveals through the
							transitions.dev panel reveal — slide, fade, and cross-blur both
							ways — instead of popping in and out of the tree.
						-->
						<div
							class="t-panel-slide w-10 rounded-full bg-surface-125 py-1 card dark:bg-surface-900"
							style="--panel-translate-y: -12px"
							data-open={workspace_open}
							inert={!workspace_open}
						>
							<div class="flex w-full flex-col items-center">
								<button
									type="button"
									aria-label={`Switch to ${next.label}`}
									class="group/surface-cycle relative flex size-8 items-center justify-center rounded-full outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
									onpointerenter={move_hover}
									onpointermove={move_hover}
									onfocusin={move_hover}
									onclick={yield* CycleSurface()}
								>
									<!--
										The icon names where the press goes, not where you already
										are; the swap is the transitions.dev icon swap, both
										destinations stacked in one cell.
									-->
									<span
										class="t-icon-swap size-4"
										data-state={surface === "editor" ? "b" : "a"}
										aria-hidden="true"
									>
										<span class="t-icon" data-icon="a">
											<CodeIcon
												class="size-4 text-muted-foreground transition-colors duration-(--duration-fast) ease-in-out group-hover/surface-cycle:text-foreground motion-reduce:transition-none"
											/>
										</span>
										<span class="t-icon" data-icon="b">
											<MessageCircle
												class="size-4 text-muted-foreground transition-colors duration-(--duration-fast) ease-in-out group-hover/surface-cycle:text-foreground motion-reduce:transition-none"
											/>
										</span>
									</span>
								</button>
							</div>
						</div>
					</div>
				{/snippet}
			</DropdownHoverSurface>
		</div>

		<div class="absolute inset-x-0 bottom-2 flex justify-center">
			<SidebarIdentity bind:open={account_open} />
		</div>
	</div>

	<main
		class="h-full min-h-0 max-h-full min-w-0 w-0 flex-1 p-2 pl-0"
		style="padding-bottom: max(0.5rem, env(safe-area-inset-bottom));"
	>
		<div
			class="docs-responsive-surfaces flex h-full min-h-0 flex-row items-stretch justify-between gap-2 overflow-visible"
		>
			<section
				class="relative flex min-h-0 min-w-0 flex-1 flex-col rounded-3xl bg-linear-to-b from-surface-125 to-surface-75 p-1 card dark:from-surface-900 dark:to-surface-925"
				data-thread-rail={rail_open}
			>
				{#if header}
					<!-- Web only, so the band sits inside the rounded card and needs its own
					     breathing room; the desktop strip titles from the window frame instead
					     and anchors flush to the card's left edge with no inset at all. -->
					<div class="flex h-10 shrink-0 items-center">
						<div class="flex w-full min-w-0 items-center px-6">
							{@render header()}
						</div>
					</div>
				{/if}
				<div class="min-h-0 flex-1">
					{@render primary()}
				</div>
				<!-- The left margin reveals complete history on proximity on every thread surface. -->
				{#if rail_open}
					<ThreadHoverRail
						header_inset={header !== undefined}
						suppressed={account_open || inspecting_image}
						{threads}
					/>
				{/if}
			</section>

			{#if secondary}
				<section
					class="min-h-0 w-(--inspector-width) shrink-0 rounded-3xl bg-linear-to-b from-surface-125 to-surface-75 p-1 card dark:from-surface-900 dark:to-surface-925"
				>
					{@render secondary()}
				</section>
			{/if}
		</div>
	</main>
</div>

<CommandMenu bind:open={command_open} {threads} />
