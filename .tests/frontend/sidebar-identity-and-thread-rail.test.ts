import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { weekly_reset_duration } from "../../modules/frontend/src/lib/identity/weekly-reset";
import { ThreadInspectorFitsBesideRail } from "../../modules/frontend/src/lib/root/shell-layout";

import { ReadStylesheets } from "./stylesheet-source";

const read = (path: string) => readFileSync(resolve(path), "utf8");

describe("sidebar identity and thread rail regressions", () => {
	it("fans provider usage reads out through Effect concurrency without a component queue", () => {
		const identity = read("modules/frontend/src/routes/components/sidebar-identity.svelte");
		const usage_controller = read(
			"modules/frontend/src/lib/identity/engine-usage-controller.ts",
		);

		expect(Effect.forkScoped).toBeTypeOf("function");
		expect(identity).toContain("usage_controller.Load(engine_id, { force })");
		expect(identity).toContain("Effect.forEach(");
		expect(identity).toContain('{ concurrency: "unbounded", discard: true }');
		expect(identity).not.toContain("client.GetEngineUsage");
		expect(usage_controller).toContain("Effect.uninterruptibleMask((restore)");
		expect(usage_controller).toContain("Effect.forkIn(CompleteFlight");
		expect(usage_controller).not.toContain("Queue.unbounded");
	});

	/**
	 * The enabled engine set is known from the catalog before any provider
	 * answers, so the menu paints one named row per enabled engine from the
	 * first frame — skeleton meters until that engine's first report lands —
	 * in stable catalog order, instead of an anonymous skeleton that later
	 * re-resolves into named sections in provider-answer order. The fan-out
	 * itself starts at mount, so an open usually shows readings straight away.
	 */
	it("renders every enabled engine as a named row from the first frame", () => {
		const identity = read("modules/frontend/src/routes/components/sidebar-identity.svelte");
		const usage = read("modules/frontend/src/routes/components/sidebar-engine-usage.svelte");

		expect(identity).toContain("The fan-out starts at mount, not at first open");
		expect(identity).toContain("engine_ids={usage_engine_ids}");
		expect(usage).toContain("const engine_rows = $derived(");
		/** Catalog order keys the rows; answer order never reshuffles them. */
		expect(usage).toContain("{#each engine_rows as row, row_index (row.engine_id)}");
		expect(usage).toContain('{:else if row.kind === "pending"}');
		expect(usage).toContain("usage loading");
		/** The identity is real even while the reading is pending. */
		expect(usage).toContain("{EngineDisplayName(row.engine_id)}");
		/** A menu with only pending rows must not claim nothing is connected. */
		expect(usage).toContain("engine_rows.length === 0");
		expect(usage).toContain('{#if usage_state.status === "loaded"}');
	});

	it("keeps unavailable provider errors out of the provider header", () => {
		const usage = read("modules/frontend/src/routes/components/sidebar-engine-usage.svelte");
		const unavailable_row = usage.slice(usage.lastIndexOf("{:else}"));

		expect(unavailable_row).toContain(
			'<span class="truncate text-xs font-medium text-foreground">{engine.display_name}</span>',
		);
		expect(unavailable_row).toContain('{engine.failure ?? "Usage is unavailable right now."}');
		expect(unavailable_row).not.toContain("— usage unavailable");
	});

	it("shows the latest trustworthy weekly reset on each shader-glass provider menu", () => {
		const identity = read("modules/frontend/src/routes/components/sidebar-identity.svelte");
		const usage = read("modules/frontend/src/routes/components/sidebar-engine-usage.svelte");
		const now = Date.parse("2026-07-31T12:00:00.000Z");

		expect(
			weekly_reset_duration(
				[
					{
						id: "session",
						kind: "session",
						percent_used: 10,
						resets_at: "2026-08-07T12:00:00.000Z",
					},
					{
						id: "weekly",
						kind: "weekly",
						percent_used: 20,
						resets_at: "2026-08-01T12:00:00.000Z",
					},
					{
						id: "model-weekly",
						kind: "weekly",
						percent_used: 30,
						resets_at: "2026-08-02T12:00:00.000Z",
					},
				],
				now,
			),
		).toBe("2 days");
		expect(
			weekly_reset_duration(
				[
					{
						id: "weekly",
						kind: "weekly",
						percent_used: 20,
						resets_at: "2026-07-31T17:00:00.000Z",
					},
				],
				now,
			),
		).toBe("5 hours");
		expect(
			weekly_reset_duration(
				[
					{
						id: "weekly",
						kind: "weekly",
						percent_used: 20,
						resets_at: "2026-07-31T12:45:00.000Z",
					},
				],
				now,
			),
		).toBe("45 minutes");
		expect(
			weekly_reset_duration(
				[
					{
						id: "weekly",
						kind: "weekly",
						percent_used: 20,
						resets_at: "2026-08-01T12:00:00.000Z",
					},
					{ id: "model-weekly", kind: "weekly", percent_used: 30 },
				],
				now,
			),
		).toBeUndefined();
		expect(
			weekly_reset_duration(
				[
					{
						id: "weekly",
						kind: "weekly",
						percent_used: 20,
						resets_at: "2026-07-31T11:00:00.000Z",
					},
				],
				now,
			),
		).toBeUndefined();

		expect(usage).toContain("weekly_reset_duration(engine.windows, checked_at_ms)");
		expect(identity).toContain('<ShaderGlassSurface class="w-full rounded-2xl">');
		expect(identity).toContain("bg-transparent! p-0! shadow-none! ring-0!");
		expect(usage).toContain('<div class="flex flex-col px-1 py-1">');
		expect(usage).not.toContain("flex flex-col gap-2.5 px-1 py-1");
		expect(usage).toContain('<DropdownMenuSeparator class="my-1" />');
		expect(usage).toContain(
			'Your weekly limit resets in <span class="text-foreground">{weekly_reset}</span>.',
		);
	});

	it("yields sidebar motion token reads through the browser DOM boundary", () => {
		const motion = read("modules/frontend/src/lib/identity/usage-window-motion.ts");
		const usage = read("modules/frontend/src/routes/components/sidebar-engine-usage.svelte");

		expect(motion).toContain('import { RunBrowserDom } from "$lib/browser/dom"');
		expect(motion).toContain("export const MotionEasing = () =>");
		expect(motion).toContain("export const MotionDuration = () =>");
		expect(motion).toContain("yield* RunBrowserDom(() =>");
		expect(usage).toContain("const motion_duration = yield* MotionDuration();");
		expect(usage).toContain("const motion_easing = yield* MotionEasing();");
	});

	it("keeps thread-list rows stationary while retaining proximity reveal", () => {
		const rail = read("modules/frontend/src/routes/components/thread-hover-rail.svelte");
		const styles = ReadStylesheets();

		expect(rail).toContain('import { RunBrowserDom } from "$lib/browser/dom"');
		expect(rail).toContain("onpointermove={yield* TrackPointer(event)}");
		/**
		 * Both gestures dismiss the card. The wheel one is what keeps a card that
		 * now takes pointer events from swallowing a scroll: it has no scrollable
		 * ancestor of its own, so without this the gesture would move nothing.
		 */
		expect(rail).toContain("onscrollcapture={hide_card}");
		expect(rail).toContain("onwheelcapture={hide_card}");
		expect(rail).toContain("data-open={open}");
		expect(rail).toContain("inert={!open}");
		expect(rail).toContain('class="t-panel-slide-x');
		expect(rail).not.toContain("SetShifts");
		expect(rail).not.toContain("getComputedStyle");
		expect(rail).not.toContain("t-avatar");
		expect(styles).not.toContain("--avatar-");
		expect(styles).not.toContain(".t-avatar");
	});

	it("keeps the revealed list open across row navigation while the pointer stays in the band", () => {
		const rail = read("modules/frontend/src/routes/components/thread-hover-rail.svelte");

		/**
		 * Opening a thread resets focus off the zone — the router refocuses after
		 * every navigation — while the pointer is usually still resting in the
		 * band, about to open the next thread. Focus loss may only conceal once
		 * the pointer has genuinely left; proximity owns the reveal otherwise.
		 */
		expect(rail).toContain("onfocusout={yield* ConcealOnFocusOut(event)}");
		expect(rail).not.toContain("onfocusout={yield* Conceal()}");
		expect(rail).toContain("last_pointer_x = event.clientX;");
		expect(rail).toContain("zone_element?.contains(next)");
		expect(rail).toContain("if (!inside) yield* Conceal();");
	});

	it("stands the proximity rail down while the account menu covers its band", () => {
		const identity = read("modules/frontend/src/routes/components/sidebar-identity.svelte");
		const panel = read("modules/frontend/src/routes/components/sectioned-panel.svelte");
		const rail = read("modules/frontend/src/routes/components/thread-hover-rail.svelte");
		const command_menu = read("modules/frontend/src/routes/components/command-menu.svelte");

		expect(identity).toContain("open = $bindable(false)");
		expect(identity).not.toContain("let open = $state(false);");
		expect(panel).toContain("<SidebarIdentity bind:open={account_open} />");
		expect(panel).toContain("suppressed={account_open || inspecting_image}");
		expect(rail).toContain("suppressed = false,");
		expect(rail).toContain("const open = $derived(near && !suppressed);");
		expect(rail).toContain("yield* ReconcileSuppression(suppressed, near);");
		expect(rail).toContain("if (is_suppressed && proximity_active) near = false;");
		expect(rail).toMatch(/if \(suppressed\) \{\s*yield\* Conceal\(\);\s*return;\s*\}/u);
		expect(rail).toContain("if (suppressed) return;");
		expect(rail).toContain("data-open={open}");
		expect(command_menu).toContain('event.key !== "k" || (!event.metaKey && !event.ctrlKey)');
	});

	/**
	 * The list belongs to the margin or it does not appear. The floating glass
	 * card that used to stand in for a margin too narrow to seat a row painted
	 * over the transcript it was summoned from; the shell answers that window by
	 * dropping the inspector column instead, which gives the margin its room
	 * back.
	 */
	it("keeps the thread list in the margin instead of floating it over the transcript", () => {
		const rail = read("modules/frontend/src/routes/components/thread-hover-rail.svelte");
		const styles = ReadStylesheets();

		/**
		 * The list itself never floats: no second branch renders it as a card over
		 * the transcript when the margin is thin. Glass is allowed in this file —
		 * the pinned working group is a card, and wears the app's own — so the
		 * check is that the reveal panel has no fallback twin, not that the
		 * surface is unused. The per-row hover card is the one deliberate float:
		 * a single instance, never a second rendering of the list.
		 *
		 * It is inert until the reader has actually engaged a row, and only then
		 * takes pointer events, so it can be reached and read without a card
		 * nobody summoned intercepting clicks meant for the transcript.
		 */
		expect(rail).not.toContain("card_element");
		expect(rail).toContain("absolute top-0 left-full z-40 ml-0.5 w-64");
		expect(rail).toContain('card_engaged ? "pointer-events-auto" : "pointer-events-none"');
		/** Clickable to the thread, but never a second tab stop for the same one. */
		expect(rail).toContain('tabindex="-1"');
		/**
		 * The band owns the stacking context every card inside it is ordered by,
		 * so it is the value that decides whether the hover card can overlay the
		 * composer or slides beneath it. Raising the card alone cannot.
		 */
		expect(rail).toContain("absolute inset-y-0 left-1 z-30");
		expect(rail.match(/aria-label="All threads"/gu) ?? []).toHaveLength(1);
		expect(rail).toMatch(
			/class="t-panel-slide-x pointer-events-auto[^"]*"[\s\S]*?aria-label="All threads"/u,
		);
		expect(rail).not.toContain("@min-[9rem]:hidden");
		expect(rail).toContain("@min-[9rem]:flex");
		/**
		 * The band is the entire left margin — the column is offset to hand it
		 * over — and is spelled from the same tokens that offset it, so a looser
		 * prose width narrows the band instead of overrunning the transcript. It
		 * measures inside the card's padding, the same box the column's margin is
		 * measured from, and stops a gap short of the column so the composer
		 * card's edge never lands over a row.
		 */
		expect(rail).toContain(
			"calc(100%_-_0.5rem_-_var(--prose-width)_-_var(--prose-gutter)_-_var(--prose-rail-gap))",
		);
		expect(styles).toContain("--prose-rail-gap: 1rem;");
		expect(rail).not.toContain("48rem");
	});

	it("shifts the reading column off centre so its left margin feeds the rail", () => {
		const styles = ReadStylesheets();
		const panel = read("modules/frontend/src/routes/components/sectioned-panel.svelte");
		const workspace = read("modules/frontend/src/routes/components/thread-workspace.svelte");
		const composer = read("modules/frontend/src/routes/components/thread-composer.svelte");

		/** Only where the rail actually mounts, held open or revealed alike. */
		expect(panel).toContain("data-thread-rail={rail_open}");
		expect(panel).toContain(
			'const rail_open = $derived(thread_rail !== "hidden" && (!threads_loaded || threads.length > 0));',
		);
		expect(styles).toContain("--prose-gutter: 2rem;");
		/**
		 * The margin is derived from what has to fit in it, not chosen. A round
		 * 21rem left the band 4rem short of the inspector, so the rail's cards
		 * could never match the width of the card facing them across the column.
		 */
		expect(styles).toContain("--inspector-width: clamp(16rem, 25vw, 350px);");
		expect(styles).toContain("--prose-rail-inset: 2rem;");
		expect(styles).toMatch(
			/--prose-rail-margin: calc\(\s*var\(--inspector-width\) \+ var\(--prose-rail-gap\) \+ var\(--prose-rail-inset\)\s*\);/u,
		);
		/** The inspector reads the same token rather than repeating the clamp. */
		expect(panel).toContain("w-(--inspector-width)");
		/** Nested inside the column's own utility, so the card's flag reaches it. */
		expect(styles).toContain('[data-thread-rail="true"] & {');
		expect(styles).toMatch(
			/max\(\s*var\(--prose-gutter\),\s*calc\(100% - var\(--prose-width\) - var\(--prose-rail-margin\)\)\s*\)/u,
		);
		/** A card narrower than the column must not buy its gutter with a negative margin. */
		expect(styles).toMatch(/clamp\(\s*0px,[\s\S]*?calc\(100% - var\(--prose-width\)\)\s*\);/u);
		/** The transcript and the composer are one column and must move together. */
		expect(workspace).toContain("prose-column w-full max-w-(--prose-width)");
		expect(composer).toContain("prose-column pointer-events-auto w-full max-w-(--prose-width)");
		expect(workspace).not.toContain("mx-auto w-full max-w-(--prose-width)");
		expect(composer).not.toContain("mx-auto w-full max-w-(--prose-width)");
		/**
		 * An offset column measures from its own frame's edges, so the composer's
		 * frame has to be the transcript's box exactly: the inset it used to carry
		 * is a width bound on the card instead.
		 */
		expect(composer).toContain('<div class="prose-column-frame ');
		expect(composer).not.toContain("px-4 pb-4 sm:px-6 sm:pb-6");
		expect(styles).toContain(".prose-column-frame & {");
		expect(styles).toContain("max-width: min(var(--prose-width), calc(100% - 3rem));");
	});

	/**
	 * The rail's list was the only surface on either side of the transcript that
	 * was not a card: bare text on the background, fading out at the right edge
	 * through a mask, with no highlight under the pointer. It now wears what the
	 * environment card facing it across the column wears.
	 */
	it("dresses the rail's list as the card facing it across the column", () => {
		const rail = read("modules/frontend/src/routes/components/thread-hover-rail.svelte");
		const environment = read(
			"modules/frontend/src/routes/components/thread-environment-card.svelte",
		);

		/** The same glass, spelled the same way, so neither can be restyled alone. */
		const glass =
			"radius-surface min-h-0 max-h-full w-full max-w-(--inspector-width) shrink [--radius-gap:var(--spacing)] [--radius-surface:var(--radius-xl)]";
		expect(rail).toContain(glass);
		expect(environment).toContain(
			"radius-surface min-h-0 max-h-full shrink [--radius-gap:var(--spacing)] [--radius-surface:var(--radius-xl)]",
		);
		/**
		 * The card's own inner padding, which is what the glass edge is measured
		 * from, and the flex column that lets the list inside it scroll.
		 */
		expect(rail).toMatch(
			new RegExp(
				`${glass.replaceAll(/[.*+?^${}()|[\]\\]/gu, "\\$&")}"[\\s\\S]*?<div class="flex min-h-0 min-w-0 flex-col p-1">`,
				"u",
			),
		);

		/** One pill slides between rows rather than each row fading its own in. */
		expect(rail).toContain("{#snippet thread_rows(move_hover: (event: Event) => void)}");
		expect(rail).toContain("onpointerenter={RowHover(move_hover, thread)}");
		expect(rail).toContain("{@render thread_rows(move_hover)}");

		/** Rows share the environment card's shape so the pill fits them exactly. */
		expect(rail).toContain("relative flex min-w-0 items-center gap-2 rounded-lg px-2 py-2");

		/** Ellipsis, not a mask: a title that ends in "…" ended, one that fades ran off. */
		expect(rail).not.toContain("mask-r-from-85%");
		expect(rail).toContain('<span class="min-w-0 flex-1 truncate">{thread.title}</span>');
		expect(rail).toMatch(
			/<span class="min-w-0 truncate text-foreground"\s*>\{thread\.title\}/u,
		);
	});

	it("keeps the thread inspector present beside every transcript", () => {
		const layout = read("modules/frontend/src/routes/+layout.svelte");

		/** One derivation answers for both the column and the desktop frame's spacer. */
		expect(layout).toContain('const thread_inspector_open = $derived(surface === "threads")');
		expect(layout).not.toContain("bind:innerWidth={viewport_width}");
		expect(layout).toContain("{#if inspector_open}");
		expect(layout).not.toContain('{#if surface === "editor" || is_thread}');

		/** A wide window seats both; the screenshot-sized window seats only one. */
		expect(ThreadInspectorFitsBesideRail(1600, "balanced")).toBe(true);
		expect(ThreadInspectorFitsBesideRail(1387, "balanced")).toBe(true);
		expect(ThreadInspectorFitsBesideRail(1386, "balanced")).toBe(false);
		expect(ThreadInspectorFitsBesideRail(1250, "balanced")).toBe(false);
		/** The threshold follows the reading column both ways. */
		expect(ThreadInspectorFitsBesideRail(1259, "tight")).toBe(true);
		expect(ThreadInspectorFitsBesideRail(1258, "tight")).toBe(false);
		expect(ThreadInspectorFitsBesideRail(1518, "loose")).toBe(true);
		expect(ThreadInspectorFitsBesideRail(1517, "loose")).toBe(false);
	});

	/**
	 * Every thread surface gets the same history on the same terms. Proximity is
	 * the resting behaviour, and the reader can pin it open — but that is a stored
	 * preference about how they read, never something a route decides for them.
	 */
	it("reveals the thread list on hover across root and thread routes", () => {
		const layout = read("modules/frontend/src/routes/+layout.svelte");
		const panel = read("modules/frontend/src/routes/components/sectioned-panel.svelte");
		const rail = read("modules/frontend/src/routes/components/thread-hover-rail.svelte");

		expect(layout).toContain("const is_thread_route = $derived(");
		expect(layout).toContain("const is_conversation_route = $derived(");
		expect(layout).toContain("const is_workspace_route = $derived(");
		expect(layout).toContain(
			'const is_new_thread_route = $derived(page.url.pathname === "/" || is_workspace_route);',
		);
		/**
		 * The route still decides whether the margin carries the list at all; only
		 * the stored preference decides whether it has to be asked for.
		 */
		expect(layout).toContain("is_conversation_route || is_new_thread_route");
		expect(layout).toContain('? "proximity" : "hidden"');
		expect(layout).toContain("{thread_rail}");
		expect(panel).toContain('thread_rail: "hidden" | "proximity";');
		expect(panel).toContain(
			'thread_rail !== "hidden" && (!threads_loaded || threads.length > 0)',
		);
		/**
		 * An unloaded list must not close the rail: doing so drew "we do not know
		 * your threads" exactly like "you have none", and took the working group
		 * down with the hover group.
		 */
		expect(panel).toContain("threads_loaded: boolean;");
		expect(layout).toContain("const threads_loaded = $derived(catalog_state.threads_loaded);");
		expect(layout).toContain("{threads_loaded}");
		expect(panel).toContain("{#if rail_open}");
		expect(panel).not.toContain('{#if surface === "threads" && threads.length > 0}');
		expect(rail).toContain("const open = $derived(near && !suppressed);");
		expect(rail).toContain("data-open={open}");
		expect(rail).toContain("inert={!open}");
		expect(rail).toContain("TrackPointer");
		/**
		 * The list is revealed by proximity and nothing else. A pin control once
		 * lived here, added against a misdiagnosis of threads not rendering; it
		 * answered a question nobody had asked and is gone.
		 */
		expect(rail).not.toContain("onpin");
		expect(rail).not.toContain("aria-pressed={pinned}");
		expect(rail).not.toContain("AppearancePreferences");
		expect(panel).not.toContain("onpinthreadrail");
		expect(layout).not.toContain("thread_rail_pinned");
		const preferences = read("modules/frontend/src/lib/runtime/appearance-preferences.ts");
		expect(preferences).not.toContain("thread_rail_pinned");
		/** The inspector column belongs to every concrete conversation. */
		expect(layout).toContain('const thread_inspector_open = $derived(surface === "threads")');
	});

	it("keeps the rail's live rows honest and bounded", () => {
		const rail = read("modules/frontend/src/routes/components/thread-hover-rail.svelte");
		const navigation = read("modules/frontend/src/lib/root/thread-navigation.ts");

		/**
		 * A finished thread does not drop to the list the moment its run ends —
		 * that is exactly when there is a reason to look at it. The pinned group
		 * holds working *and* unread; the list is what neither describes.
		 */
		expect(rail).toContain("PinnedThreads(threads)");
		expect(rail).toContain("SettledThreads(threads)");
		expect(rail).toContain("ThreadIsAwaitingAnswer(thread)");
		expect(rail).toContain("thread.engine_id, thread.model_id");
		expect(rail).not.toMatch(/\b(?:mock_)?(?:narration|timer)\b/iu);
		expect(rail).not.toMatch(/\b(?:setInterval|setTimeout)\b/u);

		/** Forge's cursor is the only unread state; development controls never fabricate it. */
		expect(rail).toContain("ThreadNeedsAttention(thread)");
		expect(rail).not.toContain("unread_ids");
		expect(rail).toContain('"--state-dot-tone: var(--unread)"');
		expect(rail).toContain('"--state-dot-tone: var(--destructive)"');
		expect(rail).toContain('ThreadFailed(thread) ? "Unread failure" : "Unread"');
		expect(rail).toContain('"Question awaiting answer"');

		/** The normal and working regions share one ordinary flex stack and real gap. */
		expect(rail).toContain('working_threads.length > 0 ? "gap-4" : ""');
		const working_region = rail.match(
			/class="pointer-events-auto[^"]*"\s+aria-label="Working"/u,
		)?.[0];
		expect(working_region).toBeDefined();
		/** Both rail cards answer to the inspector's width, not to the band's. */
		expect(working_region).toContain("max-w-(--inspector-width)");
		expect(working_region).not.toContain("absolute");
		/**
		 * The group is as tall as the threads in it, and the cap that stops it
		 * lives inside the card's padding: a fade on the wrapper or the glass eats
		 * the card's edge and takes the shadow with it.
		 */
		expect(working_region).not.toContain("docs-scroll-fade");
		expect(working_region).not.toContain("overflow-y-auto");
		expect(rail).toMatch(
			/<ShaderGlassSurface class="t-resize t-resize-auto min-h-0 max-h-full shrink rounded-xl">\s*<div class="min-w-0 p-1">\s*<!--/u,
		);
		/**
		 * The cap is a row count, measured rather than written down as a length,
		 * and both the height and the fade only exist once there is a row past the
		 * edge to hide.
		 */
		expect(rail).toContain("const VisibleWorkingRows = 10;");
		expect(rail).toContain(
			"working_threads.length > 0 ? working_rows_height / working_threads.length : 0",
		);
		expect(rail).toContain(
			"working_row_height > 0 && working_threads.length > VisibleWorkingRows",
		);
		expect(rail).toContain(
			'class={working_scrolls ? "docs-scroll-fade overflow-x-hidden overflow-y-auto" : ""}',
		);
		expect(rail).toContain("max-height:${working_row_height * VisibleWorkingRows}px");
		expect(rail).toContain(
			'<div class="flex flex-col" bind:clientHeight={working_rows_height}>',
		);
		/** No hardcoded height anywhere between the glass and the rows. */
		const working_card = rail.match(
			/<ShaderGlassSurface class="t-resize t-resize-auto min-h-0 max-h-full shrink rounded-xl">[\s\S]*?<DropdownHoverSurface/u,
		)?.[0];
		expect(working_card).toBeDefined();
		expect(working_card).not.toMatch(/(?:^|\s)(?:max-h-|h-)\d/u);
		expect(rail).toContain(
			'class="t-panel-slide-x pointer-events-auto flex min-h-0 flex-1 flex-col gap-3"',
		);

		/**
		 * The rail renders the authoritative threads and nothing else: no stress
		 * controls, no cloned render identities, no development-only branch.
		 */
		expect(rail).not.toContain("multiplier");
		expect(rail).not.toContain("<Slider");
		expect(rail).not.toContain("import.meta.env.DEV");
		expect(navigation).not.toContain("ThreadRailDisplayEntries");
		expect(rail).toContain("ThreadRailTimeGroups(recent_threads, now_ms)");
		expect(rail).toContain("{#each recent_thread_groups as group, group_index (group.id)}");
		expect(rail).toContain('<section class="flex flex-col">');
		/**
		 * The heading sits on the rows' own left edge inside the card's padding,
		 * and one that opens the card carries the rows' top inset itself — the
		 * group gap only spaces headers that have a group above them.
		 */
		expect(rail).toContain(
			'class={`px-2 pb-1 text-xs font-medium text-muted-foreground ${group_index === 0 ? "pt-2" : ""}`}',
		);
		/**
		 * `min-h-0` at every step from the card down to the scroll box. A flex item
		 * refuses to shrink below its content without it, so the list grew past the
		 * card and the glass clipped the overflow — a scroll box with nothing to
		 * scroll, because it was never the thing being constrained.
		 */
		expect(rail).toContain('<div class="flex min-h-0 min-w-0 flex-col p-1">');
		expect(rail).toContain(
			'class="docs-scroll-fade relative flex min-h-0 flex-col gap-3 overflow-x-hidden overflow-y-auto"',
		);
		expect(navigation).toContain('label: "Yesterday"');
		expect(navigation).toContain('label: "Last 3 days"');
		expect(navigation).toContain('label: "Last 7 days"');
		expect(navigation).toContain('label: "Past month"');
		expect(navigation).not.toContain('label: "Older"');
		expect(rail).toContain("{#each group.threads as thread (thread.thread_id)}");
		expect(rail).toContain("{#each working_threads as thread (thread.thread_id)}");
	});

	it("shows project names without status icons or marquee motion", () => {
		const rail = read("modules/frontend/src/routes/components/thread-hover-rail.svelte");
		const animations = read("modules/frontend/src/lib/styles/animations.css");

		expect(rail).toContain('thread.primary_project?.display_name ?? "No project"');
		expect(rail).toContain("ThreadIsAwaitingAnswer(thread)");
		expect(rail).toContain("--color-purple-400");
		expect(rail).toContain('"Question awaiting answer"');
		expect(rail).not.toContain("WorkingStatusLine");
		expect(animations).not.toContain("marquee-scroll");
	});

	it("clears or remeasures a hover pill when its dynamic row moves or disappears", () => {
		const hover_surface = read(
			"modules/frontend/src/routes/components/dropdown-hover-surface.svelte",
		);

		expect(hover_surface).toContain("let active_target: HTMLElement | undefined;");
		expect(hover_surface).toContain("const apply_hover = (target: HTMLElement)");
		expect(hover_surface).toContain(
			"const mutation_observer = new MutationObserver(reconcile);",
		);
		expect(hover_surface).toContain("const resize_observer = new ResizeObserver(reconcile);");
		expect(hover_surface).toContain("if (!element.contains(active_target))");
		expect(hover_surface).toContain('!active_target.matches(":hover")');
		expect(hover_surface).toContain("apply_hover(active_target);");
		expect(hover_surface).toContain("mutation_observer.disconnect();");
		expect(hover_surface).toContain("resize_observer.disconnect();");
		expect(hover_surface).toContain("onfocusout={clear_departed_focus}");
		expect(hover_surface).toContain("{@attach observe_hover_target}");
	});

	it("never offers an effective settle action for Forge-authoritative active work", () => {
		const rail = read("modules/frontend/src/routes/components/thread-hover-rail.svelte");

		expect(rail).toContain('import * as ContextMenu from "$lib/components/ui/context-menu"');
		expect(rail).toContain("{#if !ThreadHasActiveWork(thread)}");
		expect(rail).toContain("<ContextMenu.Item onclick={yield* Settle(thread)}>");
		expect(rail).toContain("<ArrowBarToDown />");
		expect(rail).toContain("ThreadHasActiveWork,");
		expect(rail).toContain("const client = yield* ArtisanClient;");
		expect(rail).toContain("if (ThreadHasActiveWork(thread) || ThreadSettled(thread)) return;");
		expect(rail).toContain('type: "thread.attention.acknowledge",');
		expect(rail).toContain("reader_activity_at: ThreadReaderActivityAt(thread),");
		/** The row stays the link it was; the trigger renders through it. */
		expect(rail).toContain("{#snippet child({ props })}");
		expect(rail).toContain("onpointermove={RowPointerMove(props, move_hover)}");
	});

	/** Reading is committed on a real departure, never on entry or same-route navigation. */
	it("settles observed root activity only when leaving its thread", () => {
		const rail = read("modules/frontend/src/routes/components/thread-hover-rail.svelte");

		expect(rail).toContain(
			"Option.getOrUndefined(ResolveThreadRoute(threads, active_route_id))",
		);
		/**
		 * A top-level yield site tracks what was genuinely visible, but the command
		 * is gated by a change in thread identity.
		 */
		expect(rail).toMatch(
			/AcknowledgeDepartedThread\(\s*active_route_id,\s*open_thread,\s*reader_is_watching,\s*inspecting_agent,/u,
		);
		expect(rail).toContain("AdvanceThreadReadTracking(read_tracking");
		expect(rail).toContain("yield* orchestration.CurrentInspection");
		expect(rail).toContain("reader_can_acknowledge_root_conversation(");
		expect(rail).toContain("if (transition.acknowledgement === undefined) return;");
		expect(rail).toContain("reader_activity_at: transition.acknowledgement.reader_activity_at");
		expect(rail).toContain("thread_id: transition.acknowledgement.thread_id,");
	});
});
