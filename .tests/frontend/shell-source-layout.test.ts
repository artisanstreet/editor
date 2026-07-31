import { readFileSync } from "node:fs";
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

	it("gates Forge loading and disconnection with a centered blurred overlay", () => {
		const overlay = Read("modules/frontend/src/routes/components/forge-connection-overlay.sv");
		const preview = Read("modules/frontend/src/routes/components/forge-shell-preview.sv");

		expect(overlay).toContain("absolute inset-0 z-50 grid place-items-center");
		expect(overlay).toContain("backdrop-blur-md");
		expect(overlay).toContain('role={presentation.tone === "error" ? "alert" : "status"}');
		expect(overlay).toContain("aria-live=");
		expect(overlay).toContain('tabindex="-1"');
		expect(overlay).toContain("document.activeElement");
		expect(overlay).toContain('querySelector<HTMLElement>("a, button")');
		expect(overlay).toContain("previous_focus.focus({ preventScroll: true })");
		expect(overlay).toContain("ForgeStartLaunchUrl");
		expect(overlay).toContain("retry_connection");
		expect(overlay).toContain("retry_hydration");
		/** A settled failure can be closed, leaving the disconnected shell browsable. */
		expect(overlay).toContain("{#if presentation.dismissible}");
		expect(overlay).toContain('aria-label="Dismiss and browse the disconnected client"');
		expect(overlay).toContain("onclick={ondismiss}");
		expect(overlay).toContain('event.key !== "Escape"');
		expect(preview).toContain("bg-linear-to-b from-surface-125 to-surface-75 p-1 card");
	});

	it("mounts the inspector for the workspace in view and keeps controls in the composer", () => {
		const layout = Read("modules/frontend/src/routes/+layout.sv");
		const thread = Read("modules/frontend/src/routes/t/[workspace]/[thread]/+page.sv");
		const thread_route = Read("modules/frontend/src/routes/components/thread-route.sv");
		const thread_panel = Read("modules/frontend/src/routes/components/thread-panel.sv");
		const thread_workspace = Read("modules/frontend/src/routes/components/thread-workspace.sv");
		const composer = Read("modules/frontend/src/routes/components/thread-composer.sv");
		const model_selector = Read(
			"modules/frontend/src/routes/components/model-selector/view.sv",
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
		expect(thread).toContain("Thread · Artisan Editor");
		expect(thread).toContain("{#key `${page.params.workspace}:${thread_id}`}");
		expect(thread).toContain("<ThreadRoute {thread_id} />");
		expect(thread_route).toContain("<ThreadWorkspace");
		expect(thread_panel).not.toContain("<ModelSelector");
		expect(composer).toContain("<ModelSelector");
		expect(composer).toContain("onpolicychange");
		expect(model_selector).toContain('aria-label="Select model"');
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
		expect(composer).toContain('"Stop current run"');
		expect(composer).toContain("PlayerStopFilled");
		expect(composer).toContain("composer-send rounded-[calc(var(--composer-radius)-0.5rem)]");
		expect(composer).not.toContain("card-glass");
		expect(composer).not.toContain("bg-white/50");
		/**
		 * The send/stop icons cross-fade through the shared t-icon-swap
		 * transition, so the reduced-motion guard is the stylesheet media query
		 * that zeroes it rather than a per-icon utility class.
		 */
		expect(composer).toContain('data-state={run_active ? "b" : "a"}');
		expect(composer).toContain("@media (prefers-reduced-motion: reduce)");
		expect(composer).toMatch(
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
		expect(controller).toContain("goto(canonical_path");
		expect(controller).toContain("replaceState: true");
		expect(controller).toContain("const thread_scope = yield* Scope.make()");
		expect(controller).toContain("ResolveThreadRoute(threads, route_id)");
		expect(controller).toContain("Scope.close(thread_scope, Exit.void)");
		expect(controller).toContain("Queue.offerUnsafe(action_queue");
		expect(controller).not.toContain(".unsafeOffer(");
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
		expect(workspace).toContain("Effect.tryPromise(() => tick())");
		expect(workspace).toContain("Effect.forkScoped");
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
		 * and it renders only while no detail below is live — a running command
		 * or streaming text is its own status.
		 */
		expect(work_session).not.toContain("artisan-working-sprite");
		expect(work_session).not.toContain('Effect.sleep("2 seconds")');
		expect(work_session).not.toContain("thinking_word_index");
		expect(work_session).toContain("thinking_word_for(item.id)");
		expect(work_session).not.toContain("EngineMarkFor");
		expect(work_session).not.toContain("engine-working-spin");
		expect(work_session).toContain(
			"{#if is_working && !has_live_detail && !has_live_status_detail}",
		);
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
		expect(workspace).toContain("has_live_detail={conversation_work_is_live(block.details)}");
		expect(workspace).not.toContain("engine_id={policy?.engine_id}");
	});

	it("keeps the rail as the entire sidebar with logo, command, surface, and marketplace controls", () => {
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
		 * command menu, the surface cycle, and the marketplace all belong to this
		 * edge, so they read as a pill rather than as circles that happen to align.
		 */
		expect(panel).toContain("rounded-full bg-surface-125 py-1 card");
		/** The Barekey mark replaces the toggle and doubles as the home link. */
		expect(panel).toContain("$lib/assets/barekey/logo-40.png");
		expect(panel).toContain("docs-sidebar-logo-mark");
		expect(panel).toMatch(/<a\s+href="\/"/);
		/** The command menu takes over the navigation the flyout used to carry. */
		expect(panel).toContain('aria-label="Open command menu"');
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
		 * project, the draft's chosen project, or the editor's own path workspace —
		 * never a fallback to "some attached project". Cycling carries that
		 * workspace into the editor URL and returns to the exact page it left.
		 */
		const layout = Read("modules/frontend/src/routes/+layout.sv");
		const identity = Read("modules/frontend/src/lib/editor/workspace-identity.ts");
		expect(layout).toContain(
			'EditorWorkspaceId(page.url.searchParams.get("workspace") ?? undefined)',
		);
		expect(layout).toContain("ResolveThreadRoute(threads, active_route_thread_id)");
		expect(layout).toContain("$draft_thread_project?.project_id");
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
		expect(menu).toContain("event.metaKey || event.ctrlKey");
		/**
		 * New thread is a plain jump into the draft route: no dropdown, no
		 * project picking, and no durable thread creation from the menu.
		 */
		/**
		 * The draft lives at `/threads`, not `/threads/new`, so no thread whose
		 * route id happens to be "new" can ever be shadowed by the draft route.
		 */
		expect(menu).toContain('Navigate("/threads")');
		expect(menu).not.toContain("/threads/new");
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
		expect(home).toContain('href="/threads"');
		expect(home).toContain(">New thread</span>");
		expect(home).not.toMatch(/WelcomePage|ThreadWorkspace|SettingsPage|LiveWorkspaceStore/);
	});

	it("locks engine switching only during an active run and routes it through policy", () => {
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.sv");
		const composer = Read("modules/frontend/src/routes/components/thread-composer.sv");
		const selector = Read("modules/frontend/src/routes/components/model-selector/view.sv");
		const engine_section = Read(
			"modules/frontend/src/routes/components/model-selector/engine-section.sv",
		);

		expect(workspace).toContain("const engine_locked = $derived(run_active);");
		expect(workspace).not.toContain("run_active || snapshot.items.length > 0");
		expect(composer).toContain(
			"<ModelSelector {disabled} {engine_locked} {policy} {onpolicychange} />",
		);
		expect(selector).toContain("engine_id: model.engine,");
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

		/** Forge owns the set, so every client opens the picker to the same order. */
		expect(selector).toContain("client.GetModelFavorites");
		expect(selector).toContain("client.UpdateModelFavorite");
		/** Favorites sort within the active engine, never across engine tabs. */
		expect(selection).toContain("models.filter((model) => model.engine === engine)");
		expect(selection).toContain("favorites.indexOf(left.id) - favorites.indexOf(right.id)");
		/** A muted outline until starred; gold is what starring earns. */
		expect(model_list).toContain('import Star from "@tabler/icons-svelte/icons/star"');
		expect(model_list).toContain(
			'import StarFilled from "@tabler/icons-svelte/icons/star-filled"',
		);
		expect(model_list).toContain("aria-pressed={favorited}");
		expect(model_list).toContain("self-center");
		expect(model_list).toContain('<Star class="size-4" aria-hidden="true" />');
		expect(model_list).toContain('<StarFilled class="size-4 text-favorite"');
		expect(model_list).not.toContain("text-favorite/");
		/** A star reads as gold, and the theme carries a value for each mode. */
		const global_styles = Read("modules/frontend/src/lib/styles/global.css");
		expect(global_styles).toContain("--color-favorite: var(--favorite);");
		expect(global_styles.match(/^\t--favorite: oklch/gm)?.length ?? 0).toBe(2);
		/** Nothing can be starred with no Forge to record it. */
		expect(selector).toContain("IsOfflineRuntimeCatalog(runtime_catalog)");
		expect(model_list).toContain("{#if favorites_available}");
	});

	it("surfaces the thread's project at the top of the thread panel and assigns it there", () => {
		const panel = Read("modules/frontend/src/routes/components/thread-panel.sv");

		expect(panel).toContain('aria-label="Thread project"');
		expect(panel).toContain('{project?.display_name ?? "No project"}');
		expect(panel).toContain("ResolveThreadRoute(threads, route_id)");
		expect(panel).toContain('type: "thread.project.assign"');
		expect(panel).toContain("client.ListProjectDirectories");
		expect(panel).toContain("client.SelectProjectDirectory");
		/** Projects already in use are picked from the header select... */
		expect(panel).toContain("onValueChange={RequestProject}");
		expect(panel).toContain("value={candidate.project_id}");
		expect(panel).toContain("value={BROWSE_VALUE}");
		/** ...which wears the same glass-and-hover-pill dropdown as the composer selects. */
		expect(panel).toContain("<DropdownHoverSurface");
		expect(panel).toContain("{@attach FollowHighlight(move_hover)}");
		/** ...and the dialog is now only the folder browser that starts a new one. */
		expect(panel).toContain("<Dialog.Title>Choose a folder</Dialog.Title>");
		/** On the draft route the same picker edits the client-side draft project. */
		expect(panel).toContain("draft_thread_project.set(candidate)");
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
