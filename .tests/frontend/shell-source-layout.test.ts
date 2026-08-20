import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import { ReadStylesheets } from "./stylesheet-source";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("Barekey docs shell reset", () => {
	it("lets the layout compose page surfaces through snippets", () => {
		const layout = Read("modules/frontend/src/routes/+layout.svelte");
		const panel = Read("modules/frontend/src/routes/components/sectioned-panel.svelte");

		expect(layout).toContain("<SectionedPanel");
		/** The layout owns route-derived state; the panel is handed the result. */
		expect(layout).toContain("{surface}");
		expect(layout).toContain("{#snippet primary()}");
		expect(layout).toContain("{@render children()}");
		expect(layout).not.toContain("workspace_catalog.RefreshProjects");
		expect(layout).not.toContain("workspace_catalog.RefreshThreads");
		expect(layout).toContain("{#if ForgeShellIsMounted(forge_gate)}");
		expect(layout).toContain("<ForgeShellPreview />");
		expect(layout).toContain("<ForgeConnectionOverlay");
		expect(layout).toContain("inert={ForgeShellIsBlocked(forge_gate)}");
		expect(layout).toContain("ondismiss={DismissGate}");
		expect(layout).not.toContain("<ForgeConnectionBanner");
		expect(panel).toContain("primary: Snippet");
		expect(panel).toContain("secondary?: Snippet");
		/** The rail is the entire sidebar; no flyout snippet travels through the panel. */
		expect(panel).not.toContain("sidebar: Snippet");
	});

	it("gates Forge loading and disconnection with the Artisan banner overlay", () => {
		const overlay = Read(
			"modules/frontend/src/routes/components/forge-connection-overlay.svelte",
		);
		const preview = Read("modules/frontend/src/routes/components/forge-shell-preview.svelte");

		/** Opaque and full-cover: the banner is the whole scene. */
		expect(overlay).toContain("absolute inset-0 z-50 flex");
		expect(overlay).toContain("bg-background");
		expect(overlay).not.toContain("backdrop-blur");
		/** The mark is the Sigurd wordmark component, which owns its own img semantics. */
		expect(overlay).toContain("<ArtisanLogo");
		const logo = Read("modules/frontend/src/lib/components/artisan-logo.svelte");
		expect(logo).toContain('aria-label="Artisan"');
		/** The mark and recovery copy share the same left edge. */
		expect(overlay).toContain("w-full max-w-xl flex-col items-start");
		/** Progress phases shimmer the banner; the reassurance line waits out 5s. */
		expect(overlay).toContain("banner-shimmer");
		expect(overlay).toContain("animate-[reassurance-in_500ms_var(--ease-in-out)_5s_forwards]");
		expect(overlay).toContain("This is taking more time than expected…");
		/** Settled failures use the structured crash layout with real actions. */
		expect(overlay).toContain("Artisan Editor ran into a problem and could not continue.");
		expect(overlay).toContain("What happened?");
		expect(overlay).toContain("What to do now");
		expect(overlay).toContain("PresentForgePairingGuidance");
		expect(overlay).toContain('class="font-mono text-xs text-muted-foreground"');
		expect(overlay).toContain("{presentation.failure.code}");
		expect(overlay).not.toContain(">Error code<");
		expect(overlay).not.toContain(">Diagnostic<");
		expect(overlay).not.toContain("presentation.failure.diagnostics.protocol_code");
		expect(overlay).not.toContain("presentation.failure.diagnostics.attempts");
		expect(overlay).toContain('role={presentation.tone === "error" ? "alert" : "status"}');
		expect(overlay).toContain("aria-live=");
		expect(overlay).toContain('tabindex="-1"');
		expect(overlay).toContain("document.activeElement");
		expect(overlay).toContain('querySelector<HTMLElement>("a, button")');
		expect(overlay).toContain(
			"yield* RunBrowserDom(() => previous_focus?.focus({ preventScroll: true }))",
		);
		expect(overlay).toContain("ForgeStartLaunchUrl");
		expect(overlay).toContain("retry_connection");
		expect(overlay).toContain("retry_hydration");
		/** A settled failure can be closed, leaving the disconnected shell browsable. */
		expect(overlay).toContain("{#if presentation.dismissible}");
		expect(overlay).toContain('aria-label="Dismiss and browse the disconnected client"');
		expect(overlay).toContain("onclick={yield* ondismiss}");
		expect(overlay).toContain("onkeydown={yield* DismissOnEscape(event)}");
		expect(preview).toContain("bg-linear-to-b from-surface-125 to-surface-75 p-1 card");
	});

	it("mounts the inspector for the workspace in view and keeps controls in the composer", () => {
		const layout = Read("modules/frontend/src/routes/+layout.svelte");
		const thread = Read("modules/frontend/src/routes/t/[workspace]/[thread]/+page.svelte");
		const thread_route = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const thread_panel = Read("modules/frontend/src/routes/components/thread-panel.svelte");
		const thread_workspace = Read(
			"modules/frontend/src/routes/components/thread-workspace.svelte",
		);
		const composer = Read("modules/frontend/src/routes/components/thread-composer.svelte");
		const composer_controls = Read(
			"modules/frontend/src/routes/components/composer/controls.svelte",
		);
		const model_selector = Read(
			"modules/frontend/src/routes/components/model-selector/view.svelte",
		);
		const policy_controls = Read(
			"modules/frontend/src/routes/components/model-selector/policy-controls.svelte",
		);

		expect(layout).toContain("/^\\/t\\/[^/]+\\/[^/]+\\/?$/");
		/** The workspace's own route is a thread surface too: same chrome, no transcript yet. */
		expect(layout).toContain("/^\\/t\\/[^/]+\\/?$/");
		expect(layout).toContain(
			"const is_thread_route = $derived(is_conversation_route || is_workspace_route)",
		);
		expect(layout).toContain("<ThreadPanel");
		expect(layout).toContain("thread_id={active_thread?.thread_id}");
		expect(layout).toContain("workspace_id={active_workspace_id}");
		/**
		 * The same column also carries the editor's files; the thread is one of
		 * two. Only the thread's inspector answers to the window's width — the
		 * editor's file tree has no proximity rail to make room for.
		 */
		expect(layout).toContain('secondary={surface === "editor"');
		expect(layout).toContain("? editor_files");
		expect(layout).toContain('secondary={surface === "editor" ? editor_files : secondary}');
		expect(thread).not.toContain("<svelte:head>");
		expect(thread).toContain("{#key `${page.params.workspace}:${thread_id}`}");
		expect(thread).toContain("<ThreadRouteGate {thread_id} />");
		/** The window names the thread the way the rail does: mode-resolved, rename first. */
		expect(thread_route).toContain(
			'{thread === undefined ? "Thread" : thread_display_title(thread, $thread_title_mode)}',
		);
		expect(thread_route).toContain("<ThreadWorkspace");
		expect(thread_panel).not.toContain("<ModelSelector");
		expect(composer).toContain("<ComposerControls");
		expect(composer_controls).toContain("<ModelSelector");
		expect(composer_controls).toContain("onpolicychange");
		expect(model_selector).toContain('aria-label="Select model"');
		/**
		 * The trigger reads as the model's own row does: its lab mark, its name,
		 * its supported effort, and its speed only when that is not the default.
		 */
		expect(model_selector).toContain('class="flex min-w-0 items-center gap-2"');
		expect(model_selector).toContain(
			"{@const trigger_mark = ProviderMarkFor(selected_model?.definition.provider)}",
		);
		expect(model_selector).toContain(
			'<TriggerMark class={EngineMarkClass(trigger_mark, "size-4")} />',
		);
		expect(model_selector).toContain("trigger_speed_presentation.label");
		expect(model_selector).toContain("speed_option_presentation(selected)");
		expect(policy_controls).not.toContain("speed_option_presentation");
		expect(policy_controls).toContain("{speed.label}");
		expect(model_selector).toContain(
			"if (selected === undefined || selected.default) return undefined;",
		);
		expect(model_selector).not.toContain("BoltFilled");

		/**
		 * That label changes width as the model, effort, and speed change. Anchored
		 * to the trigger's trailing edge, the picker slid sideways on every pick, so
		 * the anchor is the leading edge, which does not move.
		 */
		expect(model_selector).toContain('align="start"');
		expect(model_selector).not.toContain('align="end"');
		/**
		 * Open and close are the transitions.dev dropdown, driven by the popover's
		 * own `data-state`; the primitive's stock keyframes are turned off so the
		 * two do not animate the same element at once.
		 */
		expect(model_selector).toContain("t-dropdown");
		expect(model_selector).toContain("animate-none!");
		expect(model_selector).toContain("t-resize t-resize-auto");
		expect(model_selector).toContain('thinking.availability === "supported"');
		expect(model_selector).toContain("thinking_level_labels[selected_thinking_level]");
		expect(model_selector).toContain('class="text-muted-foreground"');

		/**
		 * The shader panel wears `.t-dropdown` too and orchestrates it through its
		 * own scoped classes, so the popover's copy stays qualified by the popover
		 * attribute rather than claiming the bare class for everyone.
		 */
		const styles = ReadStylesheets();

		expect(styles).toContain('.t-dropdown[data-popover-content][data-state="open"]');
		expect(styles).toContain('.t-dropdown[data-popover-content][data-state="closed"]');
		expect(styles).not.toMatch(/^\.t-dropdown \{/mu);
		expect(styles).toContain("interpolate-size: allow-keywords;");
		expect(styles).toContain("@media (prefers-reduced-motion: reduce)");
		/**
		 * The chevron's colour rides on the element. It used to live in a sibling
		 * stylesheet pulled in with `<style src>`, which loads no rules here — so
		 * the rule never applied, and the model list's scrollbar stayed Chromium's
		 * stock one for the same reason.
		 */
		expect(model_selector).toContain(
			'class="pointer-events-none size-3.5 shrink-0 text-muted-foreground"',
		);
		expect(model_selector).not.toContain("<style src=");
		expect(model_selector).not.toContain("model-trigger-chevron");
		/** Provider marks live in one shared module so every surface agrees. */
		const engine_presentation = Read("modules/frontend/src/lib/engine/presentation.ts");
		expect(model_selector).toContain("EngineMarkFor(harness.id)");
		expect(engine_presentation).toContain(
			'claude: { accent: "#d97757", icon: SvglClaudeAILogo, monochrome: false }',
		);
		expect(engine_presentation).toContain(
			'codex: { accent: "#10a37f", icon: SvglOpenAILogo, monochrome: true }',
		);
		expect(engine_presentation).toContain(
			'grok: { accent: "#6b7280", icon: SvglGrokLogo, monochrome: true }',
		);
		expect(model_selector).toContain("service_tier");
		expect(model_selector).toContain("<PolicyControls");
		expect(policy_controls).toContain("oncontext");
		expect(policy_controls).toContain("onpermission");
		expect(policy_controls).toContain("onspeed");
		expect(policy_controls).toContain("onthinking");
		expect(model_selector).not.toContain("workflow_mode");
		expect(model_selector).not.toContain("Toggle workflow mode");
		expect(model_selector).not.toMatch(/>Build<|>Plan</);
		expect(composer).not.toContain('aria-label="Add images"');
		expect(composer_controls).toContain('"Stop current run"');
		expect(composer_controls).toContain("PlayerStopFilled");
		/**
		 * The send face's corner is the composer's own, minus the padding between
		 * them — derived once on the card, not recomputed here against a step that
		 * is not its parent.
		 */
		expect(composer_controls).toContain("composer-send rounded-(--radius-nested)");
		expect(composer).toContain("radius-surface [--radius-surface:var(--radius-2xl)]");
		expect(composer).toContain("[--radius-gap:calc(var(--spacing)*2)]");
		expect(ReadStylesheets()).toContain(
			"--radius-nested: calc(var(--radius-surface) - var(--radius-gap, 0px));",
		);
		expect(composer).not.toContain("card-glass");
		expect(composer).not.toContain("bg-white/50");
		/**
		 * The send/stop icons cross-fade through the shared t-icon-swap
		 * transition, so the reduced-motion guard is the stylesheet media query
		 * that zeroes it rather than a per-icon utility class.
		 */
		expect(composer_controls).toContain('data-state={run_active ? "b" : "a"}');
		expect(ReadStylesheets()).toMatch(/@utility t-icon-swap \{[\s\S]*?& \.t-icon \{/u);
		expect(composer).toContain("yield* Cancel");
		expect(thread_route).toContain('work?.status === "running"');
		expect(thread_route).toContain('work?.status === "waiting"');
		expect(thread_route).toContain('payload: { type: "run.cancel" }');
		expect(thread_route).toContain("onabort={CancelRun}");
		expect(thread_route).not.toContain("RefreshAuthoritativeThread");
		expect(thread_route).toContain("RefreshInteractionContext");
		expect(thread_workspace).toContain("fold_resolved_approvals_into_work");
		expect(thread_workspace).toContain('block.item.type === "approval"');
		expect(thread_workspace).toContain('block.item.state !== "requested"');
		expect(composer).not.toContain('aria-label="Use voice input"');
	});

	it("owns conversation subscriptions and snapshots by route identity", () => {
		const route = Read("modules/frontend/src/routes/t/[workspace]/[thread]/+page.svelte");
		const controller = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const interaction = Read("modules/frontend/src/lib/thread-interaction/commands.ts");
		const accepted_command = interaction.indexOf("const result = yield* command;");
		const accepted_reconciliation = interaction.indexOf(
			"yield* after_acceptance(result).pipe(Effect.ignore);",
			accepted_command,
		);
		const sender_reconciliation = controller.indexOf(
			"RefreshInteractionContext.pipe(Effect.forkIn(thread_scope), Effect.asVoid)",
		);
		const interaction_refresh = controller.indexOf(
			"steering_settlement: AwaitSteeringAcknowledged(receipt.command_id)",
			sender_reconciliation,
		);

		expect(route).toContain("const thread_id = $derived(page.params.thread)");
		expect(route).toContain("page.params.workspace");
		expect(route).toContain("{#key `${page.params.workspace}:${thread_id}`}");
		expect(controller).toContain("navigation.Navigate(canonical_path");
		expect(controller).toContain("replaceState: true");
		expect(controller).toContain("const thread_scope = yield* Scope.make()");
		expect(controller).toContain("readonly thread_open:");
		expect(controller).not.toContain("client.GetThreadOpen(route_id)");
		expect(controller).not.toContain("ResolveThreadRoute(threads, route_id)");
		expect(controller).toContain("Scope.close(thread_scope, Exit.void)");
		expect(controller).toContain("const draft_thread = yield* DraftThreadController");
		expect(controller).toContain("AwaitPendingSubmissionClaim(thread_id)");
		expect(controller).not.toContain("Queue.offerUnsafe(action_queue");
		expect(controller).toContain("Effect.forkIn(");
		expect(controller).toContain("RunAuthoritativeSubscription(");
		expect(controller).toContain("client.Events.pipe(");
		expect(controller).toContain('Stream.debounce("50 millis")');
		expect(controller).toContain("update.batch.thread_id !== thread_id");
		expect(controller).toContain("update.batch.conversation_id !== conversation_id");
		expect(controller).toContain("!CanReplaceConversationSnapshot(snapshot, next)");
		expect(controller).toContain("yield* SubmitDurableCommand(");
		expect(controller).toContain("AwaitCanonicalUserMessage");
		expect(controller).toContain("receipt.command_id");
		expect(controller).toContain("ConversationUserMessageWithSourceReference");
		expect(accepted_command).toBeGreaterThan(-1);
		expect(accepted_reconciliation).toBeGreaterThan(accepted_command);
		expect(sender_reconciliation).toBeGreaterThan(-1);
		expect(interaction_refresh).toBeGreaterThan(sender_reconciliation);
	});

	it("positions loaded threads at the bottom and promotes a local turn to the top inset", () => {
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");
		const message = Read("modules/frontend/src/routes/components/conversation-message.svelte");

		expect(workspace).toContain("bind:viewportRef={viewport}");
		expect(workspace).toContain(
			"const PositionLoadedThread = (view_state: ConversationViewState | undefined) =>",
		);
		expect(workspace).toContain("Effect.promise(() => tick())");
		expect(workspace).toContain("if (anchor_layout_revision > 0) yield* UpdateAnchorLayout");
		expect(workspace).toContain("ConversationBottomScrollTop(");
		expect(workspace).toContain("ConversationUserMessageWithSourceReference(");
		expect(workspace).toContain("ConversationEndSpaceHeight(");
		expect(workspace).toContain("pending_user_message_reference = undefined");
		expect(workspace).toContain("outcome.user_message_reference");
		expect(workspace).toContain("ConversationUserMessageWithSourceReference");
		expect(workspace).toContain('"smooth"');
		expect(message).toContain("data-conversation-item-id={item.id}");
	});

	it("renders active work as one muted shimmering word trailing the flow", () => {
		const work_session = Read(
			"modules/frontend/src/routes/components/conversation-work-session.svelte",
		);
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");

		/**
		 * Disclosure belongs to the header, not to settlement. Gating it on
		 * `!is_working` while the header itself showed during a run left the trace
		 * forced open with no control to shut it.
		 */
		expect(work_session).toContain("work_session_disclosure({");
		expect(work_session).toContain("data-open={disclosure.data_open}");
		expect(work_session).toContain("data-state={disclosure.data_state}");
		/**
		 * The handover lands in the header, never as its own transcript entry —
		 * which is why the header is unconditional: an entry that arrived before
		 * the header did had nowhere else to sit. One header element serves both
		 * disclosure states, so it hosts the handover exactly once.
		 */
		expect(work_session.match(/<ConversationStatus item=\{transition\}/gu) ?? []).toHaveLength(
			1,
		);
		/** Only a live session ticks; settled history never enters its bound loop. */
		expect(work_session).toContain("while (ended_at === undefined) {");
		expect(work_session).toContain("{#if disclosure.can_collapse}");
		expect(work_session).toContain("<button");
		/**
		 * The header is one element for the session's life: it owns the entrance
		 * and the divider's growth, and swapping it for a sibling when the first
		 * detail arrived replayed both. Disclosure appears inside the element that
		 * is already mounted, never by rebuilding it.
		 */
		expect(work_session).not.toContain("{:else if shows_header}");
		expect(
			work_session.match(/t-settle-underline relative flex w-full items-center/gu) ?? [],
		).toHaveLength(1);
		expect(work_session).toContain(
			'? `${duration_kind === "worked" ? "Working" : "Thinking"} for',
		);
		expect(work_session).toContain('role="status"');
		/**
		 * The generic sprite, the verb carousel, and the engine mark are gone. The
		 * muted thinking line — the model's latest summary, or a verb when it has
		 * none — trails the flow instead of pinning above it. A shimmering tool
		 * chain and a streamed reply each own progress while visible, so neither
		 * gets a redundant quiet-status row beneath it.
		 */
		expect(work_session).not.toContain("artisan-working-sprite");
		expect(work_session).not.toContain('Effect.sleep("2 seconds")');
		expect(work_session).not.toContain("thinking_word_index");
		expect(work_session).toContain(
			"thinking_word_for(item.id, thinking_visibility_generation)",
		);
		expect(work_session).toContain("ReconcileThinkingVisibility(renders_status_line)");
		expect(work_session).not.toContain("EngineMarkFor");
		expect(work_session).not.toContain("engine-working-spin");
		expect(work_session.replace(/\s+/gu, " ")).toContain(
			"steering_pending || (!has_live_reply && !has_live_status_detail && !waiting_for_activity)",
		);
		expect(work_session).toContain("{#if renders_status_line}");
		/**
		 * Trace prose and the opening wait use the same half-rem gap beneath the
		 * divider as the header label uses above it.
		 */
		const panel_gap_matches_header =
			work_session.includes("group-data-[open=true]/session-acc:pt-2") ||
			work_session.includes("padding-top: 0.5rem;");
		expect(panel_gap_matches_header).toBe(true);
		/** The line is one clipped row whether it says a verb or a summary sentence. */
		expect(work_session).toContain("my-2 flex max-w-(--prose-body-width) items-center");
		expect(work_session).toContain('class="trace-command-label min-w-0 truncate"');
		expect(work_session).toContain(
			"t-settle-underline relative flex w-full items-center justify-between gap-3 pb-2",
		);
		/** Entrances are CSS mount animations: directives stall the async tree. */
		expect(work_session).not.toMatch(/\s(?:in|out|transition):[A-Za-z]/);
		expect(ReadStylesheets()).toContain("@keyframes status-swap-enter");
		/** The divider grows from the measured label width out to the edge. */
		expect(work_session).toContain("bind:clientWidth={label_width}");
		expect(ReadStylesheets()).toContain("@keyframes settle-underline-grow");
		expect(work_session).toContain('is_failed ? "text-destructive" : ""');
		expect(work_session).toContain("hidden={disclosure.details_hidden}");
		expect(workspace).toContain("has_live_reply={conversation_reply_is_live(block.details)}");
		expect(workspace).toContain(
			"waiting_for_activity={conversation_waiting_for_activity(block.details)}",
		);
		/**
		 * The engine came back as a word, never as a mark. Before anything has
		 * come back there is no thought to name, so the status says which side the
		 * wait is on; the provider's turn-start swaps it to the session's own word,
		 * even when the resulting thought remains private.
		 * The icon and the spinner stay gone.
		 */
		expect(workspace).toContain("engine_id={policy?.engine_id}");
		expect(work_session).toContain("? active_work_label_for({");
		expect(work_session).toContain("thinking_visibility_generation,");
		expect(work_session).toContain("waiting_for_activity,");
		expect(work_session).toContain("item.responded_at !== undefined || has_visible_details");
		expect(work_session).toContain("is_working ? status_label : label");
		/** A handoff run is answered by the engine it handed off to. */
		expect(work_session).toContain("$derived(transition?.target_engine_id ?? engine_id)");
	});

	it("keeps the rail as the entire sidebar with logo, new-thread, surface, and marketplace controls", () => {
		const panel = Read("modules/frontend/src/routes/components/sectioned-panel.svelte");
		const sidebar_styles = ReadStylesheets();

		/**
		 * The rail is the whole sidebar now: no expanded panel behind it, so
		 * nothing toggles and none of the shadcn sidebar machinery remains.
		 */
		expect(panel).not.toContain("Sidebar.Root");
		expect(panel).not.toContain("Sidebar.Trigger");
		expect(panel).not.toContain("<LayoutSidebar");
		expect(sidebar_styles).not.toContain('[data-slot="sidebar"]');
		expect(sidebar_styles).not.toContain(".t-rail-control");
		/**
		 * The rail's controls share one vertical housing: the brand mark, the
		 * new-thread action, the surface cycle, and the marketplace all belong to this
		 * edge, so they read as a pill rather than as circles that happen to align.
		 */
		expect(panel).toContain("rounded-full bg-surface-125 py-1 card");
		/** The canonical Artisan star replaces the toggle and doubles as the home link. */
		expect(panel).toContain("$lib/assets/barekey/artisan-star.svg");
		expect(panel).toContain("src={artisan_star}");
		expect(panel).toContain("group-hover/artisan-logo:opacity-100");
		expect(panel).toContain("card-plastic");
		expect(panel).toMatch(/<a\s+href="\/"/);
		/** The rail's first action starts a fresh draft conversation. */
		expect(panel).toContain('aria-label="New thread"');
		expect(panel).toContain('href="/"');
		expect(panel).toContain("yield* PrepareNewThreadDraft(new_thread_key)");
		expect(panel).toContain("yield* navigation.Navigate(new_thread_path)");
		expect(panel).toContain("<MessagePlus");
		expect(panel).toContain("<CommandMenu bind:open={command_open} {threads} />");
		expect(Read("modules/frontend/src/routes/components/command-menu.svelte")).toContain(
			"yield* PrepareNewThreadDraft(new_thread_draft_key(undefined))",
		);
		expect(panel).toContain('aria-label="Marketplace"');
		expect(panel).toContain("<ShoppingBag");
		/**
		 * The surface cycle is its own group under the pill, shown only while a
		 * workspace is open for the editor, and it reveals through the
		 * transitions.dev panel reveal rather than mounting and unmounting.
		 */
		expect(panel).toContain('data-state={surface === "editor" ? "b" : "a"}');
		expect(panel).toContain('<span class="t-icon" data-icon="a">');
		expect(panel).toContain('<span class="t-icon" data-icon="b">');
		expect(panel).toContain("<Code");
		expect(panel).toContain("<MessageCircle");
		expect(panel).toContain("aria-label={`Switch to ${next.label}`}");
		expect(panel).toContain("data-open={workspace_open}");
		expect(panel).toContain("inert={!workspace_open}");
		expect(panel).toContain("t-panel-slide");
		expect(sidebar_styles).toMatch(
			/@utility t-panel-slide \{[\s\S]*?&\[data-open="true"\] \{/u,
		);
		/** One reduced-motion authority, keyed on the duration tokens everything reads. */
		expect(sidebar_styles).toMatch(
			/@media \(prefers-reduced-motion: reduce\) \{[\s\S]*?transition-duration: 1ms !important;/u,
		);
		/**
		 * The workspace is what the current route is inside — the open thread's
		 * authoritative project, or the URL's own workspace segment resolved
		 * against Forge's catalog — never a route assertion taken on trust or
		 * "some attached project". Cycling carries that workspace into the editor
		 * URL and returns to the exact thread it left.
		 */
		const layout = Read("modules/frontend/src/routes/+layout.svelte");
		const identity = Read("modules/frontend/src/lib/editor/workspace-identity.ts");
		expect(layout).not.toContain('searchParams.get("workspace")');
		expect(layout).toContain("return active_thread.primary_project;");
		expect(layout).toContain("ResolveThreadRoute(threads, active_route_thread_id)");
		expect(layout).toContain('ThreadWorkspaceId(page.params.workspace ?? "")');
		expect(layout).toContain(
			"return projects.find((candidate) => candidate.project_id === route_workspace_id);",
		);
		expect(layout).toContain("active_project?.project_id");
		expect(layout).toContain("workspace_id={active_workspace_id}");
		/** A new thread starts in the project you are in; outside one it is the picker. */
		expect(panel).toContain("WorkspaceRoutePath(workspace_id)");
		expect(panel).toContain("href={new_thread_path}");
		expect(identity).not.toContain("projects[0]");
		expect(panel).toContain("EditorRoutePath(");
		expect(panel).toContain("ThreadRoutePath(workspace_id, thread_id)");
	});

	it("uses the Barekey docs gradient card surface for page content", () => {
		const panel = Read("modules/frontend/src/routes/components/sectioned-panel.svelte");
		const global_styles = ReadStylesheets();

		expect(panel).toContain(
			"rounded-3xl bg-linear-to-b from-surface-125 to-surface-75 p-1 card",
		);
		expect(panel).not.toContain("bg-background");
		expect(global_styles).toContain('--font-sans: "Artisan Neo", sans-serif;');
	});

	it("carries thread navigation and the draft-thread quick link in the command menu", () => {
		const menu = Read("modules/frontend/src/routes/components/command-menu.svelte");
		const home = Read("modules/frontend/src/routes/+page.svelte");

		expect(menu).toContain("<CommandDialog");
		expect(menu).toContain("!event.metaKey && !event.ctrlKey");
		expect(menu).toContain("onkeydown={yield* ToggleCommandMenu(event)}");
		/**
		 * New thread is a plain jump to the root draft: no dropdown, no
		 * project picking, and no durable thread creation from the menu. The
		 * dedicated `/threads` draft route no longer exists.
		 */
		expect(menu).toContain('href="/"');
		expect(menu).not.toContain('"/threads"');
		expect(menu).toContain("<span>New thread</span>");
		expect(menu).not.toContain("client.CreateThread");
		expect(menu).not.toContain("client.ListProjectDirectories");
		expect(menu).not.toContain("client.SelectProjectDirectory");
		expect(menu).not.toContain('type: "thread.create"');
		expect(menu).not.toContain('type: "thread.project.assign"');
		expect(menu).not.toContain("artisanDesktop");
		expect(menu).toContain("ProjectScopedThreadGroups(threads)");
		expect(menu).toContain('project?.display_name ?? "Unassigned"');
		expect(menu).toContain("ThreadRoutePathFor(thread)");
		/** The layout owns the retained live catalog; the menu only renders its projection. */
		const layout = Read("modules/frontend/src/routes/+layout.svelte");
		expect(layout).toContain("yield* WorkspaceCatalogController");
		expect(layout).toContain("workspace_catalog.SubscribeProjects");
		expect(layout).toContain("workspace_catalog.SubscribeThreadList");
		expect(layout).toContain("workspace_catalog.Changes");
		expect(layout).not.toContain("client.ListProjects");
		expect(layout).not.toContain("client.ListThreads");
		/** Ready mounts from retained catalog state; subscriptions own snapshots and recovery. */
		expect(layout).not.toContain("workspace_catalog.RefreshProjects");
		expect(layout).not.toContain("workspace_catalog.RefreshThreads");
		expect(layout).toContain("session_defaults.Refresh.pipe(Effect.ignore, Effect.forkScoped)");
		expect(layout).toContain("if (!IsCurrentForgeHydration(forge_gate, generation)) return;");
		expect(layout).toContain("catalog_state = yield* workspace_catalog.Current;");
		expect(menu).not.toContain("ArtisanClient");
		/**
		 * The root page is a new thread and nothing else — it holds no picker of
		 * its own, because the project is a word inside the sentence the composer
		 * sits under. Both it and a routed workspace mount the same surface; the
		 * only difference is where that word's answer comes from.
		 */
		expect(existsSync(resolve("modules/frontend/src/routes/threads/+page.svelte"))).toBe(false);
		expect(home).toContain("<NewThreadRoute />");
		expect(home).not.toMatch(/ProjectOrbit|OrbitProjectsFor|onopen=/u);
		expect(home).not.toContain('"/threads"');
		expect(home).not.toMatch(/WelcomePage|ThreadWorkspace|SettingsPage|LiveWorkspaceStore/);
		expect(
			existsSync(resolve("modules/frontend/src/routes/components/project-orbit.svelte")),
		).toBe(false);
		expect(existsSync(resolve("modules/frontend/src/lib/root/project-orbit.ts"))).toBe(false);

		const workspace = Read("modules/frontend/src/routes/components/new-thread-route.svelte");
		expect(Read("modules/frontend/src/routes/t/[workspace]/+page.svelte")).toContain(
			"<NewThreadRoute {workspace_id} />",
		);
		expect(workspace).toContain("<ThreadComposer");
		expect(workspace).toContain("<ProjectSelector");
		expect(workspace).toContain("SubmitFirstMessage");
		expect(workspace).toContain("yield* SubmitNewThreadDraft(draft_key, submission)");
		expect(workspace).toContain("{#key draft_revision}");
		expect(workspace).toContain(
			"NavigateCreatedDraft(\n\t\t\t\tThreadRoutePath(created.project.project_id, created.thread_id)",
		);
		/** The draft is aimed at the sentence's project, off the reactive mirror. */
		expect(workspace).toContain("const current = yield* draft_thread.Current;");
		expect(workspace).toContain("SeededDraftPolicy(snapshot.catalog, snapshot.defaults)");
		/**
		 * The mirrored revision only decides when to re-align. Aligning *against*
		 * it refused every attempt while it trailed a reset, and the refusal was
		 * discarded — so the composer armed itself over a draft holding no project.
		 * The revision is read from the controller, and the answer is now read too.
		 */
		expect(workspace).toContain(
			"const expected_revision = yield* draft_thread.CurrentRevision;",
		);
		expect(workspace).toContain(
			"draft_aligned = yield* draft_thread.AlignAtRevision(expected_revision, target, policy)",
		);
		expect(workspace).not.toContain(
			"yield* draft_thread.AlignAtRevision(revision, target, policy)",
		);
		/** A send is gated on the draft holding a project, not on the sentence naming one. */
		expect(workspace).toContain(
			"const draft_ready = $derived(project !== undefined && draft_aligned);",
		);
		expect(workspace).toContain("disabled={!draft_ready || locked}");
		/**
		 * Changing the word is a plain state change where the URL says nothing
		 * about the project, and a navigation where it does — a route that named
		 * one project while composing into another would be lying.
		 */
		expect(workspace).toContain("if (workspace_id === undefined) {");
		expect(workspace).toContain("chosen_project_id = next.project_id;");
		expect(workspace).toContain("yield* Navigate(WorkspaceRoutePath(next.project_id));");
		/** Word animation spans retain real text separators for wrapping and copying. */
		expect(workspace).toContain('{#if word.leading_space}{" "}{/if}');
	});

	/**
	 * Attaching a folder is the only way a project comes into existence, so the
	 * selector's last row has to reach it. Forge owns both the filesystem and the
	 * catalog: every identity the picker holds is one Forge minted.
	 */
	it("offers a new project under the selector's own rule", () => {
		const selector = Read("modules/frontend/src/routes/components/project-selector.svelte");
		const picker = Read("modules/frontend/src/routes/components/project-folder-picker.svelte");

		expect(selector).toContain("underline decoration-muted-foreground decoration-dotted");
		expect(selector).toContain(
			'<ShaderGlassSurface strength="strong" class="rounded-2xl p-1">',
		);
		expect(selector).toContain(
			'<DropdownHoverSurface class="[--docs-sidebar-hover-radius:var(--radius-xl)]">',
		);
		expect(selector).toContain('class="pointer-events-none -mx-1 my-1 h-px bg-border/50"');
		expect(selector).toContain("New project");
		expect(selector).toContain("yield* onnewproject");

		expect(picker).toContain("client.PickProjectDirectory");
		expect(picker).toContain("client.SelectProjectDirectory({ directory_id })");
		expect(picker).not.toMatch(/root_path|artisanDesktop/u);
	});

	it("locks engine switching only during an active run and routes it through policy", () => {
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");
		const composer = Read("modules/frontend/src/routes/components/thread-composer.svelte");
		const controls = Read("modules/frontend/src/routes/components/composer/controls.svelte");
		const selector = Read("modules/frontend/src/routes/components/model-selector/view.svelte");
		const engine_section = Read(
			"modules/frontend/src/routes/components/model-selector/engine-section.svelte",
		);

		expect(workspace).toContain("const engine_locked = $derived(run_active);");
		expect(workspace).not.toContain("run_active || snapshot.items.length > 0");
		expect(composer).toContain("<ComposerControls");
		expect(controls).toContain("<ModelSelector");
		expect(controls).toContain("{engine_locked}");
		expect(controls).toContain("{runtime_catalog}");
		expect(selector).toContain("engine_id: model.engine,");
		expect(selector).toContain("model.id !== untrack(() => selected_model_id)");
		expect(selector).toContain("candidate.engine === current.engine_id");
		expect(selector).toContain("<EngineSection");
		expect(engine_section).toContain("engine_locked && engine.id !== selected_engine.id");
		expect(engine_section).toContain("finish the active run before switching engines");
	});

	/**
	 * The panel configures the model it is describing.
	 *
	 * It used to describe the hovered model while configuring the selected one,
	 * and cleared the preview as soon as the pointer reached it — so the
	 * settings snapped back to the current model on the way to touching them,
	 * and a hovered model could not be configured without being clicked first.
	 *
	 * What survives from the old rule is the half that was right: a pointer
	 * passing over a row still changes nothing. Adoption is deferred to the
	 * first deliberate act — a click on the row, or a touch of one of its
	 * settings, which cannot mean anything until that model is the one in use.
	 */
	it("configures the model the panel is describing, adopting it only on a deliberate act", () => {
		const selector = Read("modules/frontend/src/routes/components/model-selector/view.svelte");

		expect(selector).toContain("model={previewed_model}");
		expect(selector).toContain("permission_options={previewed_permissions?.options ?? []}");
		expect(selector).not.toContain("onpointerenter={yield* ResetPreview}");
		/** Hovering writes the preview and nothing else. */
		expect(selector).toContain("const PreviewModel = (model_id: string)");
		expect(selector).toMatch(
			/const PreviewModel = \(model_id: string\) =>\s*Effect\.gen\(function\* \(\) \{\s*previewed_model_id = model_id;\s*\}\);/u,
		);
		expect(
			selector.match(/if \(!\(yield\* AdoptForConfiguration\(model\)\)\) return;/gu) ?? [],
		).toHaveLength(4);
		/** Row click and control touch are the two doors, and they share one adopt. */
		expect(selector.match(/yield\* AdoptModel\(model/gmu) ?? []).toHaveLength(2);
	});

	it("stars models from the picker and floats favorites to the top of their engine", () => {
		const selector = Read("modules/frontend/src/routes/components/model-selector/view.svelte");
		const selection = Read("modules/frontend/src/lib/engine/model-selection.ts");
		const model_list = Read(
			"modules/frontend/src/routes/components/model-selector/model-list.svelte",
		);
		const composer = Read("modules/frontend/src/routes/components/thread-composer.svelte");
		const defaults_controller = Read(
			"modules/frontend/src/lib/settings/session-defaults-controller.ts",
		);

		/** Forge owns the set, so every client opens the picker to the same order. */
		expect(selector).toContain("defaults_controller.SetFavorite");
		expect(defaults_controller).toContain("client.GetModelFavorites");
		expect(defaults_controller).toContain("client.UpdateModelFavorite");
		expect(selector).not.toContain("client.ConnectionChanges");
		expect(Read("modules/frontend/src/routes/+layout.svelte")).toContain(
			"session_defaults.Refresh.pipe(Effect.ignore, Effect.forkScoped)",
		);
		/** The canonical defaults controller owns the catalog stream for every surface. */
		expect(selector).not.toContain("RuntimeCatalogChanges");
		expect(composer).not.toContain("RuntimeCatalogChanges");
		expect(composer).toContain("yield* SessionDefaultsController");
		expect(composer).toContain("defaults_controller.Changes");
		expect(composer).toContain("{runtime_catalog}");
		/** Favorites sort within the active engine, never across engine tabs. */
		expect(selection).toContain("models.filter((model) => model.engine === engine)");
		expect(selection).toContain("favorites.indexOf(left.id) - favorites.indexOf(right.id)");
		/** One stable SVG stays mounted; gold fill is what starring earns. */
		expect(model_list).toContain('import Star from "@tabler/icons-svelte/icons/star"');
		expect(model_list).not.toContain("StarFilled");
		expect(model_list).toContain("aria-pressed={favorited}");
		expect(model_list).toContain("self-center");
		expect(model_list).toContain('favorited ? "size-4 fill-current text-favorite" : "size-4"');
		expect(model_list).not.toContain("text-favorite/");
		/** A star reads as gold, and the theme carries a value for each mode. */
		const global_styles = ReadStylesheets();
		expect(global_styles).toContain("--color-favorite: var(--favorite);");
		expect(global_styles.match(/^\t--favorite: oklch/gm)?.length ?? 0).toBe(2);
		/** The stable control stays mounted but cannot be used without Forge. */
		expect(selector).toContain(
			"defaults_state?.available ?? !IsOfflineRuntimeCatalog(effective_catalog)",
		);
		expect(model_list).toContain("disabled={disabled || !favorites_available}");
		expect(model_list).not.toContain("{#if favorites_available}");
	});

	it("shows canonical plan tasks as a conditional Checklist in the thread inspector", () => {
		const panel = Read("modules/frontend/src/routes/components/thread-panel.svelte");
		const layout = Read("modules/frontend/src/routes/+layout.svelte");
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const item = Read("modules/frontend/src/routes/components/conversation-item.svelte");
		const prompt = Read("modules/frontend/src/routes/components/conversation-prompt.svelte");
		const store = Read("modules/frontend/src/lib/conversation/store.ts");

		expect(panel).toContain("flex h-full min-h-0 flex-col");
		expect(panel).toContain(
			'<ShaderGlassSurface class="t-resize t-resize-auto min-h-0 max-h-full shrink rounded-xl">',
		);
		expect(panel).toContain("min-w-0 flex-col p-1");
		expect(panel).toContain("Checklist");
		expect(panel).toContain("<ul");
		expect(panel).toContain("<li");
		expect(panel).toContain("rounded-lg px-2 py-2 text-sm");
		expect(panel).not.toContain("ArtisanClient");
		expect(panel).not.toContain("GetConversation");
		expect(panel).not.toContain("SubscribeConversation");
		expect(layout).toContain("yield* ThreadChecklist");
		expect(layout).toContain('const thread_inspector_open = $derived(surface === "threads")');
		expect(layout).toContain("<ThreadPanel");
		expect(route).toContain("checklist.Acquire(thread_id)");
		expect(route).toContain("LatestConversationPlan(snapshot.items, snapshot.turns)");
		expect(item).toContain('item.type === "plan"');
		expect(prompt).not.toContain('{ type: "plan" }');
		expect(store).toContain('if (item.type === "plan") return [];');
	});

	it("retains the complete Barekey style foundation", () => {
		const global = ReadStylesheets();

		/** The core is split by what a rule is, not by which feature it serves. */
		for (const stylesheet of [
			"theme.css",
			"utilities.css",
			"animations.css",
			"prose.css",
			"vendor.css",
		])
			expect(global).toContain(`@import "./${stylesheet}"`);
		for (const utility of [
			"inset-shadow",
			"card",
			"card-glass",
			"card-color",
			"card-lg",
			"card-diff",
		])
			expect(global).toContain(`@utility ${utility}`);
	});
});
