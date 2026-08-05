<script lang="ts" effect>
	import { Effect, Stream } from "effect";
	import type { Snippet } from "svelte";
	import type { ThreadListItem } from "@artisan/protocol";
	import CodeIcon from "@tabler/icons-svelte/icons/code";
	import MessageCircle from "@tabler/icons-svelte/icons/message-circle";
	import MessagePlus from "@tabler/icons-svelte/icons/message-plus";
	import ShoppingBag from "@tabler/icons-svelte/icons/shopping-bag";

	import barekey_logo from "$lib/assets/barekey/logo-40.png";
	import logo_gradient from "$lib/assets/barekey/logo-gradient.svg";
	import { EditorRoutePath } from "$lib/editor/workspace-identity";
	import { RouteNavigation } from "$lib/browser/route-navigation";
	import { ImageInspectionStore } from "$lib/images/inspection-store";
	import { ThreadRoutePath } from "$lib/root/thread-navigation";
	import CommandMenu from "./command-menu.sv";
	import DropdownHoverSurface from "./dropdown-hover-surface.sv";
	import SidebarIdentity from "./sidebar-identity.sv";
	import ThreadHoverRail from "./thread-hover-rail.sv";

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
	 * exactly the band the thread rail reads proximity from. Held here because
	 * the two live on opposite sides of the layout and neither owns the other.
	 */
	let account_open = $state(false);

	let {
		primary,
		secondary,
		show_thread_hover_rail,
		surface,
		thread_id,
		threads,
		workspace_id,
	}: {
		primary: Snippet;
		secondary?: Snippet;
		/** Whether the canonical conversation route owns the transcript proximity rail. */
		show_thread_hover_rail: boolean;
		/** Which workspace surface is on screen, owned by the layout. */
		surface: "editor" | "threads";
		/** The durable thread shared by both workspace surfaces. */
		thread_id: string | undefined;
		/** The live thread list, owned by the layout and shared with the command menu. */
		threads: ReadonlyArray<ThreadListItem>;
		/** The workspace the current route is inside, resolved by the layout. */
		workspace_id: string | undefined;
	} = $props();

	const workspace_open = $derived(workspace_id !== undefined && thread_id !== undefined);
	const navigation = yield* RouteNavigation;

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
	<div class="relative block h-[calc(100%-1rem)] w-14 shrink-0">
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
									class="docs-sidebar-logo relative flex size-8 items-center justify-center rounded-full outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
									onpointerenter={move_hover}
									onpointermove={move_hover}
									onfocusin={move_hover}
								>
									<!--
										The mark is a cutout: it rests as a plain foreground glyph,
										and hovering paints the brand gradient up through the logo's
										own alpha.
									-->
									<span
										aria-hidden="true"
										class="docs-sidebar-logo-mark size-5 shrink-0"
										style={`--docs-sidebar-logo-cutout: url(${barekey_logo}); --docs-sidebar-logo-gradient: url(${logo_gradient});`}
									></span>
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
									href="/"
									aria-label="New thread"
									class="group/new-thread relative flex size-8 items-center justify-center rounded-full outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
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
		class="h-[calc(100%-1rem)] min-h-0 max-h-[calc(100%-1rem)] min-w-0 w-0 flex-1 p-2 pl-0"
		style="padding-bottom: max(0.5rem, env(safe-area-inset-bottom));"
	>
		<div
			class="docs-responsive-surfaces flex h-full min-h-0 flex-row items-stretch justify-between gap-2 overflow-visible"
		>
			<section
				class="relative min-h-0 min-w-0 flex-1 rounded-3xl bg-linear-to-b from-surface-125 to-surface-75 p-1 card dark:from-surface-900 dark:to-surface-925"
			>
				{@render primary()}
				<!--
					The transcript's dead left margin doubles as a proximity reveal
					for every thread; it only exists on the threads surface, where
					that margin is real.
				-->
				{#if show_thread_hover_rail && threads.length > 0}
					<ThreadHoverRail suppressed={account_open || inspecting_image} {threads} />
				{/if}
			</section>

			{#if secondary}
				<section
					class="min-h-0 w-[clamp(16rem,25vw,350px)] shrink-0 rounded-3xl bg-linear-to-b from-surface-125 to-surface-75 p-1 card dark:from-surface-900 dark:to-surface-925"
				>
					{@render secondary()}
				</section>
			{/if}
		</div>
	</main>
</div>

<CommandMenu bind:open={command_open} {threads} />
