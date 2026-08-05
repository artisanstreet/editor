import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("Barekey docs shell reset", () => {
	it("lets the layout compose page surfaces through snippets", () => {
		const layout = Read("modules/frontend/src/routes/+layout.sv");
		const panel = Read("modules/frontend/src/routes/components/sectioned-panel.sv");

		expect(layout).toContain("<SectionedPanel");
		/** The layout owns route-derived state; the panel is handed the result. */
		expect(layout).toContain("{surface}");
		expect(layout).toContain("{#snippet primary()}");
		expect(layout).toContain("{@render children()}");
		expect(layout).toContain("client.ListProjects");
		expect(layout).toContain("client.ListThreads");
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
		const overlay = Read("modules/frontend/src/routes/components/forge-connection-overlay.sv");
		const preview = Read("modules/frontend/src/routes/components/forge-shell-preview.sv");

		/** Opaque and full-cover: the banner is the whole scene. */
		expect(overlay).toContain("absolute inset-0 z-50 flex");
		expect(overlay).toContain("bg-background");
		expect(overlay).not.toContain("backdrop-blur");
		expect(overlay).toContain('aria-label="Artisan"');
		/** The mark and recovery copy share the same left edge. */
		expect(overlay).toContain("w-full max-w-xl flex-col items-start");
		/** Progress phases shimmer the banner; the reassurance line waits out 5s. */
		expect(overlay).toContain("banner-shimmer");
		expect(overlay).toContain("late-reassurance");
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
		const layout = Read("modules/frontend/src/routes/+layout.sv");
		const thread = Read("modules/frontend/src/routes/t/[workspace]/[thread]/+page.sv");
		const thread_route = Read("modules/frontend/src/routes/components/thread-route.sv");
		const thread_panel = Read("modules/frontend/src/routes/components/thread-panel.sv");
		const thread_workspace = Read("modules/frontend/src/routes/components/thread-workspace.sv");
		const composer = Read("modules/frontend/src/routes/components/thread-composer.sv");
		const composer_controls = Read(
			"modules/frontend/src/routes/components/composer/controls.sv",
		);
		const model_selector = Read(
			"modules/frontend/src/routes/components/model-selector/view.sv",
		);
		const model_selector_styles = Read(
			"modules/frontend/src/routes/components/model-selector.css",
		);
		const policy_controls = Read(
			"modules/frontend/src/routes/components/model-selector/policy-controls.sv",
		);

		expect(layout).toContain("/^\\/t\\/[^/]+\\/[^/]+\\/?$/");
		expect(layout).toContain("<ThreadPanel />");
		/** The same column also carries the editor's files; the thread is one of two. */
		expect(layout).toContain(
			'secondary={surface === "editor" ? editor_files : is_thread ? secondary : undefined}',
		);
		expect(thread).not.toContain("<svelte:head>");
		expect(thread).toContain("{#key `${page.params.workspace}:${thread_id}`}");
		expect(thread).toContain("<ThreadRoute {thread_id} />");
		expect(thread_route).toContain(
			'<title>{thread?.title ?? "Thread"} › Artisan Editor</title>',
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
		expect(model_selector).toContain("{trigger_speed_label}");
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
		expect(model_selector_styles).toContain("color: var(--muted-foreground)");

		/**
		 * The shader panel wears `.t-dropdown` too and orchestrates it through its
		 * own scoped classes, so the popover's copy stays qualified by the popover
		 * attribute rather than claiming the bare class for everyone.
		 */
		const styles = Read("modules/frontend/src/lib/styles/global.css");

		expect(styles).toContain('.t-dropdown[data-popover-content][data-state="open"]');
		expect(styles).toContain('.t-dropdown[data-popover-content][data-state="closed"]');
		expect(styles).not.toMatch(/^\.t-dropdown \{/mu);
		expect(styles).toContain("interpolate-size: allow-keywords;");
		expect(styles).toContain("@media (prefers-reduced-motion: reduce)");
		expect(model_selector_styles).not.toContain(".model-trigger:hover .model-trigger-chevron");
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
		expect(composer_controls).toContain(
			"composer-send rounded-[calc(var(--composer-radius)-0.5rem)]",
		);
		expect(composer).not.toContain("card-glass");
		expect(composer).not.toContain("bg-white/50");
		/**
		 * The send/stop icons cross-fade through the shared t-icon-swap
		 * transition, so the reduced-motion guard is the stylesheet media query
		 * that zeroes it rather than a per-icon utility class.
		 */
		expect(composer_controls).toContain('data-state={run_active ? "b" : "a"}');
		expect(composer_controls).toContain("@media (prefers-reduced-motion: reduce)");
		expect(composer_controls).toMatch(
			/@media \(prefers-reduced-motion: reduce\) \{[\s\S]*\.t-icon-swap\) \.t-icon,/,
		);
		expect(composer).toContain("yield* Cancel");
		expect(thread_route).toContain('work?.status === "running"');
		expect(thread_route).toContain('work?.status === "waiting"');
		expect(thread_route).toContain('payload: { type: "run.cancel" }');
		expect(thread_route).toContain("onabort={CancelRun}");
		expect(thread_route).toContain("RefreshAuthoritativeThread");
		expect(thread_workspace).toContain("fold_resolved_approvals_into_work");
		expect(thread_workspace).toContain('block.item.type === "approval"');
		expect(thread_workspace).toContain('block.item.state !== "requested"');
		expect(composer).not.toContain('aria-label="Use voice input"');
	});

	it("owns conversation subscriptions and snapshots by route identity", () => {
		const route = Read("modules/frontend/src/routes/t/[workspace]/[thread]/+page.sv");
		const controller = Read("modules/frontend/src/routes/components/thread-route.sv");
		const interaction = Read("modules/frontend/src/lib/thread-interaction/commands.ts");
		const accepted_command = interaction.indexOf("const result = yield* command;");
		const accepted_reconciliation = interaction.indexOf(
			"yield* after_acceptance(result).pipe(Effect.ignore);",
			accepted_command,
		);
		const sender_reconciliation = controller.indexOf("ObserveAcceptedProjection(");
		const interaction_refresh = controller.indexOf(
			"RefreshInteractionContext.pipe(Effect.ignore)",
			sender_reconciliation,
		);

		expect(route).toContain("const thread_id = $derived(page.params.thread)");
		expect(route).toContain("page.params.workspace");
		expect(route).toContain("{#key `${page.params.workspace}:${thread_id}`}");
		expect(controller).toContain("navigation.Navigate(canonical_path");
		expect(controller).toContain("replaceState: true");
		expect(controller).toContain("const thread_scope = yield* Scope.make()");
		expect(controller).toContain("ResolveThreadRoute(threads, route_id)");
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
		expect(controller).toContain("ReconcileAcceptedUserMessage");
		expect(controller).toContain("receipt.command_id");
		expect(controller).toContain("ConversationUserMessageWithSourceReference");
		expect(accepted_command).toBeGreaterThan(-1);
		expect(accepted_reconciliation).toBeGreaterThan(accepted_command);
		expect(sender_reconciliation).toBeGreaterThan(-1);
		expect(interaction_refresh).toBeGreaterThan(sender_reconciliation);
	});

	it("positions loaded threads at the bottom and promotes a local turn to the top inset", () => {
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.sv");
		const message = Read("modules/frontend/src/routes/components/conversation-message.sv");

		expect(workspace).toContain("bind:viewportRef={viewport}");
		expect(workspace).toContain("const PositionLoadedThread = Effect.gen(function* ()");
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
			"modules/frontend/src/routes/components/conversation-work-session.sv",
		);
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.sv");

		expect(work_session).toContain(
			"const can_collapse = $derived(!is_working && has_visible_details);",
		);
		expect(work_session).toContain("{#if can_collapse}");
		expect(work_session).toContain("<button");
		expect(work_session).toContain("{:else if !is_working}");
		expect(work_session).toContain('role="status"');
		/**
		 * The generic sprite, the verb carousel, and the engine mark are gone.
		 * The muted thinking word trails the flow instead of pinning above it,
		 * and it holds across a whole tool chain — only the streamed reply below
		 * it is its own status.
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
		expect(work_session).toContain("is_working && !has_live_reply && !has_live_status_detail");
		expect(work_session).toContain("{#if renders_status_line}");
		expect(work_session).toContain('<ShimmerText class="text-base text-muted-foreground"');
		expect(work_session).toContain(
			"t-settle-underline relative flex w-full items-center justify-between gap-3 pb-2",
		);
		/** Entrances are CSS mount animations: directives stall the async tree. */
		expect(work_session).not.toMatch(/\s(?:in|out|transition):[A-Za-z]/);
		expect(work_session).toContain("@keyframes status-swap-enter");
		/** The divider grows from the measured label width out to the edge. */
		expect(work_session).toContain("bind:clientWidth={label_width}");
		expect(work_session).toContain("@keyframes settle-underline-grow");
		expect(work_session).toContain('is_failed ? "text-destructive" : ""');
		expect(work_session).toContain("hidden={!is_working && !has_visible_details}");
		expect(workspace).toContain("has_live_reply={conversation_reply_is_live(block.details)}");
		/**
		 * The engine came back as a word, never as a mark. Before anything has
		 * come back there is no thought to name, so the status says which side the
		 * wait is on; the provider's turn-start swaps it to the session's own word,
		 * even when the resulting thought remains private.
		 * The icon and the spinner stay gone.
		 */
		expect(workspace).toContain("engine_id={policy?.engine_id}");
		expect(work_session).toContain("? active_work_label_for(");
		expect(work_session).toContain("thinking_visibility_generation,");
		expect(work_session).toContain("item.responded_at !== undefined || has_visible_details");
		expect(work_session).toContain("is_working ? status_label : label");
		/** A handoff run is answered by the engine it handed off to. */
		expect(work_session).toContain("$derived(transition?.target_engine_id ?? engine_id)");
	});

	it("keeps the rail as the entire sidebar with logo, new-thread, surface, and marketplace controls", () => {
		const panel = Read("modules/frontend/src/routes/components/sectioned-panel.sv");
		const sidebar_styles = Read("modules/frontend/src/lib/styles/sidebar.css");

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
		/** The Barekey mark replaces the toggle and doubles as the home link. */
		expect(panel).toContain("$lib/assets/barekey/logo-40.png");
		expect(panel).toContain("docs-sidebar-logo-mark");
		expect(panel).toMatch(/<a\s+href="\/"/);
		/** The rail's first action starts a fresh draft conversation. */
		expect(panel).toContain('aria-label="New thread"');
		expect(panel).toContain('href="/"');
		expect(panel).toContain("<MessagePlus");
		expect(panel).toContain("<CommandMenu bind:open={command_open} {threads} />");
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
		expect(sidebar_styles).toContain('.t-panel-slide[data-open="true"]');
		expect(sidebar_styles).toMatch(
			/@media \(prefers-reduced-motion: reduce\) \{\s*\n\t\.t-panel-slide,\s*\n\t\.t-panel-slide-x \{/,
		);
		/**
		 * The workspace is what the current route is inside — the open thread's
		 * authoritative project or the draft's chosen project — never a route
		 * assertion or "some attached project". Cycling carries that workspace
		 * into the editor URL and returns to the exact thread it left.
		 */
		const layout = Read("modules/frontend/src/routes/+layout.sv");
		const identity = Read("modules/frontend/src/lib/editor/workspace-identity.ts");
		expect(layout).not.toContain('searchParams.get("workspace")');
		expect(layout).toContain("active_thread.primary_project?.project_id");
		expect(layout).toContain("ResolveThreadRoute(threads, active_route_thread_id)");
		expect(layout).toContain("draft_state.project?.project_id");
		expect(layout).toContain("workspace_id={active_workspace_id}");
		expect(identity).not.toContain("projects[0]");
		expect(panel).toContain("EditorRoutePath(");
		expect(panel).toContain("ThreadRoutePath(workspace_id, thread_id)");
	});

	it("uses the Barekey docs gradient card surface for page content", () => {
		const panel = Read("modules/frontend/src/routes/components/sectioned-panel.sv");
		const global_styles = Read("modules/frontend/src/lib/styles/global.css");

		expect(panel).toContain(
			"rounded-3xl bg-linear-to-b from-surface-125 to-surface-75 p-1 card",
		);
		expect(panel).not.toContain("bg-background");
		expect(global_styles).toContain('--font-sans: "Artisan Neo", sans-serif;');
	});

	it("carries thread navigation and the draft-thread quick link in the command menu", () => {
		const menu = Read("modules/frontend/src/routes/components/command-menu.sv");
		const home = Read("modules/frontend/src/routes/+page.sv");

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
		/** The layout owns the live list; the menu only renders what it is handed. */
		const layout = Read("modules/frontend/src/routes/+layout.sv");
		expect(layout).toContain("client.SubscribeThreadList");
		expect(layout).toContain("ApplyRootThreadListUpdate");
		expect(menu).not.toContain("ArtisanClient");
		/** The root page is the draft: the composer creates the thread on first send. */
		expect(existsSync(resolve("modules/frontend/src/routes/threads/+page.sv"))).toBe(false);
		expect(home).toContain("<ThreadComposer");
		expect(home).toContain("SubmitFirstMessage");
		expect(home).toContain("yield* draft_thread.Submit(submission)");
		expect(home).toContain("seed_model?.capabilities.speed_options.find(");
		expect(home).not.toContain("last_model?.capabilities.speed_options.find(");
		expect(home).toContain(
			"Navigate(ThreadRoutePath(created.project.project_id, created.thread_id))",
		);
		expect(home).not.toContain('"/threads"');
		expect(home).not.toMatch(/WelcomePage|ThreadWorkspace|SettingsPage|LiveWorkspaceStore/);
	});

	it("locks engine switching only during an active run and routes it through policy", () => {
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.sv");
		const composer = Read("modules/frontend/src/routes/components/thread-composer.sv");
		const controls = Read("modules/frontend/src/routes/components/composer/controls.sv");
		const selector = Read("modules/frontend/src/routes/components/model-selector/view.sv");
		const engine_section = Read(
			"modules/frontend/src/routes/components/model-selector/engine-section.sv",
		);

		expect(workspace).toContain("const engine_locked = $derived(run_active);");
		expect(workspace).not.toContain("run_active || snapshot.items.length > 0");
		expect(composer).toContain("<ComposerControls");
		expect(controls).toContain("<ModelSelector");
		expect(controls).toContain("{engine_locked}");
		expect(controls).toContain("{runtime_catalog}");
		expect(selector).toContain("engine_id: model.engine,");
		expect(selector).toContain("model.id !== untrack(() => selected_model_id)");
		expect(selector).toContain("<EngineSection");
		expect(engine_section).toContain("engine_locked && engine.id !== selected_engine.id");
		expect(engine_section).toContain("finish the active run before switching engines");
	});

	it("stars models from the picker and floats favorites to the top of their engine", () => {
		const selector = Read("modules/frontend/src/routes/components/model-selector/view.sv");
		const selection = Read("modules/frontend/src/lib/engine/model-selection.ts");
		const model_list = Read(
			"modules/frontend/src/routes/components/model-selector/model-list.sv",
		);
		const composer = Read("modules/frontend/src/routes/components/thread-composer.sv");
		const defaults_controller = Read(
			"modules/frontend/src/lib/settings/session-defaults-controller.ts",
		);

		/** Forge owns the set, so every client opens the picker to the same order. */
		expect(selector).toContain("defaults_controller.SetFavorite");
		expect(defaults_controller).toContain("client.GetModelFavorites");
		expect(defaults_controller).toContain("client.UpdateModelFavorite");
		expect(selector).toContain("client.ConnectionChanges");
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
		const global_styles = Read("modules/frontend/src/lib/styles/global.css");
		expect(global_styles).toContain("--color-favorite: var(--favorite);");
		expect(global_styles.match(/^\t--favorite: oklch/gm)?.length ?? 0).toBe(2);
		/** The stable control stays mounted but cannot be used without Forge. */
		expect(selector).toContain(
			"defaults_state?.available ?? !IsOfflineRuntimeCatalog(effective_catalog)",
		);
		expect(model_list).toContain("disabled={disabled || !favorites_available}");
		expect(model_list).not.toContain("{#if favorites_available}");
	});

	it("surfaces the thread's project at the top of the thread panel and assigns it there", () => {
		const panel = Read("modules/frontend/src/routes/components/thread-panel.sv");
		const project_selector = Read(
			"modules/frontend/src/routes/components/panel/project-selector.sv",
		);
		const picker = Read("modules/frontend/src/routes/components/project-folder-picker.sv");

		expect(panel).toContain("<ProjectSelector");
		expect(project_selector).toContain('aria-label="Thread project"');
		expect(project_selector).toContain('{project?.display_name ?? "No project"}');
		expect(panel).toContain("ResolveThreadRoute(threads, route_id)");
		expect(panel).toContain('type: "thread.project.assign"');
		/** Projects already in use are picked from the header select... */
		expect(project_selector).toContain("onValueChange={yield* onvaluechange(event)}");
		expect(project_selector).toContain("value={candidate.project_id}");
		expect(project_selector).toContain("value={BROWSE_VALUE}");
		/** ...which wears the same glass-and-hover-pill dropdown as the composer selects. */
		expect(project_selector).toContain("<DropdownHoverSurface");
		expect(project_selector).toContain("{@attach FollowHighlight(move_hover)}");
		/** ...and browsing for a new folder is the picker component's whole job. */
		expect(panel).toContain("<ProjectFolderPicker");
		expect(picker).toContain("client.ListProjectDirectories");
		expect(picker).toContain("yield* browse_requests.IsCurrent(request_epoch)");
		expect(picker).toContain("yield* MakeLatestRequestGate");
		expect(picker).toContain("const outcome = yield* Effect.result(");
		expect(picker).toContain("client.SelectProjectDirectory");
		expect(picker).toContain("<Dialog.Title>Choose a folder</Dialog.Title>");
		/**
		 * The picker is the familiar file-browser everywhere, Electron included:
		 * places sidebar, breadcrumb path, search beside it, files for context.
		 */
		expect(picker).toContain('aria-label="Places"');
		expect(picker).toContain('aria-label="Current folder path"');
		expect(picker).toContain('aria-label="Search this folder"');
		expect(picker).toContain("visible_files");
		/** On the draft route the same picker edits the client-side draft project. */
		expect(panel).toContain("yield* draft_thread.SelectProject(candidate)");
		/** Draft-ness follows the absent route id rather than a hardcoded path. */
		expect(panel).toContain("const is_draft = $derived(route_id === undefined);");
	});

	it("retains the complete Barekey style foundation", () => {
		const global = Read("modules/frontend/src/lib/styles/global.css");

		for (const stylesheet of ["sidebar.css", "prose.css", "markdown.css"])
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
