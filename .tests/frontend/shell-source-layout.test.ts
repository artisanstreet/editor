import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("Barekey docs shell reset", () => {
	it("lets the layout compose page surfaces through snippets", () => {
		const layout = Read("modules/frontend/src/routes/+layout.sv");
		const panel = Read("modules/frontend/src/routes/components/sectioned-panel.sv");

		expect(layout).toContain("<SectionedPanel {sidebar} {primary}");
		expect(layout).toContain("{#snippet sidebar()}");
		expect(layout).toContain("{#snippet primary()}");
		expect(layout).toContain("{@render children()}");
		expect(layout).toContain("client.ListProjects");
		expect(layout).toContain("client.ListThreads");
		expect(layout).toContain("{#if forge_gate.has_hydrated_shell}");
		expect(layout).toContain("<ForgeShellPreview />");
		expect(layout).toContain("<ForgeConnectionOverlay");
		expect(layout).toContain('inert={forge_gate.state.phase !== "ready"}');
		expect(layout).not.toContain("<ForgeConnectionBanner");
		expect(panel).toContain("primary: Snippet");
		expect(panel).toContain("secondary?: Snippet");
		expect(panel).toContain("sidebar: Snippet");
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
		expect(preview).toContain("bg-linear-to-b from-surface-125 to-surface-75 p-1 card");
	});

	it("mounts the inspector only for concrete thread routes and keeps controls in the composer", () => {
		const layout = Read("modules/frontend/src/routes/+layout.sv");
		const thread = Read("modules/frontend/src/routes/threads/[id]/+page.sv");
		const thread_route = Read("modules/frontend/src/routes/threads/[id]/thread-route.sv");
		const thread_panel = Read("modules/frontend/src/routes/components/thread-panel.sv");
		const composer = Read("modules/frontend/src/routes/components/thread-composer.sv");
		const model_selector = Read("modules/frontend/src/routes/components/model-selector.sv");

		expect(layout).toContain("/^\\/threads\\/[^/]+\\/?$/");
		expect(layout).toContain("<ThreadPanel />");
		expect(layout).toContain("secondary={is_thread ? secondary : undefined}");
		expect(thread).toContain("Thread · Artisan Editor");
		expect(thread).toContain("{#key thread_id}");
		expect(thread).toContain("<ThreadRoute {thread_id} />");
		expect(thread_route).toContain("<ThreadWorkspace");
		expect(thread_panel).not.toContain("<ModelSelector");
		expect(composer).toContain("<ModelSelector");
		expect(composer).toContain("onpolicychange");
		expect(model_selector).toContain('aria-label="Select model"');
		expect(model_selector).toContain("SvglOpenAILogo");
		expect(model_selector).not.toMatch(/Claude|Anthropic/);
		expect(model_selector).toContain("service_tier");
		expect(model_selector).toContain(
			'const composer_controls: ReadonlyArray<ComposerControl> = ["model"];',
		);
		expect(model_selector).not.toContain("workflow_mode");
		expect(model_selector).not.toContain("Toggle workflow mode");
		expect(model_selector).not.toMatch(/>Build<|>Plan</);
		expect(composer).not.toContain('aria-label="Add images"');
		expect(composer).toContain('"Stop current run"');
		expect(composer).toContain("PlayerStopFilled");
		expect(composer).toContain("text-white/25");
		expect(composer).toContain("motion-reduce:transition-none");
		expect(composer).toContain("yield* Cancel");
		expect(thread_route).toContain('work?.status === "running"');
		expect(thread_route).toContain('work?.status === "waiting"');
		expect(thread_route).toContain('payload: { type: "run.cancel" }');
		expect(thread_route).toContain("onabort={CancelRun}");
		expect(thread_route).toContain("RefreshAuthoritativeThread");
		expect(composer).not.toContain('aria-label="Use voice input"');
	});

	it("owns conversation subscriptions and snapshots by route identity", () => {
		const route = Read("modules/frontend/src/routes/threads/[id]/+page.sv");
		const controller = Read("modules/frontend/src/routes/threads/[id]/thread-route.sv");
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

		expect(route).toContain("const thread_id = $derived(page.params.id)");
		expect(route).toContain("{#key thread_id}");
		expect(route).toContain("goto(canonical_path");
		expect(route).toContain("replaceState: true");
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
		expect(workspace).toContain("const PositionLoadedThread = async () =>");
		expect(workspace).toContain("await tick();");
		expect(workspace).toContain("ConversationBottomScrollTop(");
		expect(workspace).toContain("ConversationUserMessageWithSourceReference(");
		expect(workspace).toContain("ConversationEndSpaceHeight(");
		expect(workspace).toContain("pending_user_message_reference = undefined");
		expect(workspace).toContain("outcome.user_message_reference");
		expect(workspace).toContain("ConversationUserMessageWithSourceReference");
		expect(workspace).toContain('"smooth"');
		expect(message).toContain("data-conversation-item-id={item.id}");
	});

	it("renders active work with the Artisan sprite and data-driven activity copy", () => {
		const work_session = Read(
			"modules/frontend/src/routes/components/conversation-work-session.sv",
		);
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.sv");

		expect(work_session).toContain(
			"const can_collapse = $derived(!is_working && has_visible_details);",
		);
		expect(work_session).toContain("{#if can_collapse}");
		expect(work_session).toContain("<button");
		expect(work_session).toContain("{:else}");
		expect(work_session).toContain('role="status"');
		expect(work_session).toContain("artisan-working-sprite.png");
		expect(work_session).toContain("thinking_word_at(thinking_word_index)");
		expect(work_session).toContain('Effect.sleep("2 seconds")');
		expect(work_session).toContain('{activity_label ?? "Working..."}');
		expect(work_session).toContain(
			'class="flex w-full items-center gap-1 border-b border-border pb-2"',
		);
		expect(work_session).toContain('class="flex w-fit items-center gap-2 py-0.5"');
		expect(work_session).toContain("<span>{label}</span>");
		expect(work_session).toContain("hidden={!is_working && !has_visible_details}");
		expect(workspace).toContain("latest_active_activity_label(block.details)");
	});

	it("matches the Barekey docs inset sidebar and circular toggle", () => {
		const panel = Read("modules/frontend/src/routes/components/sectioned-panel.sv");
		const provider = Read("modules/frontend/src/lib/components/ui/sidebar/sidebar-provider.sv");

		expect(panel).toContain(
			'style="--sidebar-width: 16rem; --sidebar-width-icon: 2.5rem; min-height: 0;"',
		);
		expect(panel).toContain('<Sidebar.Root variant="inset" collapsible="icon">');
		expect(panel).toContain("absolute right-0 top-2 hidden size-10");
		expect(panel).toContain("rounded-full bg-surface-125 card");
		expect(panel).toContain("<LayoutSidebar");
		expect(provider.indexOf("const sidebar = set_sidebar")).toBeLessThan(
			provider.indexOf("yield* Queue.unbounded"),
		);
		expect(provider).toContain("Queue.offerUnsafe(");
		expect(provider).not.toContain(".unsafeOffer(");
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

	it("keeps the copied docs identity and live creation controls in the sidebar", () => {
		const sidebar = Read("modules/frontend/src/routes/components/artisan-sidebar.sv");
		const home = Read("modules/frontend/src/routes/+page.sv");

		expect(sidebar).toContain("$lib/assets/barekey/logo-40.png");
		expect(sidebar).toContain('class="size-5 shrink-0 invert dark:invert-0"');
		expect(sidebar).toContain('<span class="font-logo">Artisan Editor</span>');
		expect(sidebar).toContain(
			'<Sidebar.Header class="h-14 justify-center pl-6 pr-14 lg:pl-2">',
		);
		expect(sidebar).toContain("client.CreateThread");
		expect(sidebar).not.toContain('type: "thread.create"');
		expect(sidebar).not.toContain("artisanDesktop");
		expect(sidebar).toContain("client.ListProjectDirectories");
		expect(sidebar).toContain("client.SelectProjectDirectory");
		expect(sidebar).toContain("Select a project folder");
		expect(sidebar).not.toContain("Folder selection is available in the Artisan desktop app.");
		expect(sidebar).not.toContain('type: "thread.project.assign"');
		expect(sidebar).toContain("project_id: project.project_id");
		expect(sidebar).not.toContain("No existing projects");
		expect(sidebar).not.toContain("<Sidebar.GroupLabel>Threads</Sidebar.GroupLabel>");
		expect(sidebar).toContain("ProjectScopedThreadGroups(threads)");
		expect(sidebar).toContain("<Sidebar.MenuSub");
		expect(sidebar).toContain("<Sidebar.MenuSubButton");
		expect(sidebar).toContain('project?.display_name ?? "Unassigned"');
		expect(sidebar).toContain("ThreadRoutePath(thread.thread_id)");
		expect(sidebar).toContain("client.SubscribeThreadList");
		expect(sidebar).toContain("ApplyRootThreadListUpdate");
		expect(sidebar).toContain(
			"isActive={page.url.pathname === ThreadRoutePath(thread.thread_id)}",
		);
		expect(home).not.toMatch(/WelcomePage|ThreadWorkspace|SettingsPage|LiveWorkspaceStore/);
	});

	it("retains the complete Barekey style foundation", () => {
		const global = Read("modules/frontend/src/lib/styles/global.css");

		for (const stylesheet of ["sidebar.css", "prose.css", "markdown.css"])
			expect(global).toContain(`@import "./${stylesheet}"`);
		for (const utility of ["inset-shadow", "card", "card-color", "card-lg", "card-diff"])
			expect(global).toContain(`@utility ${utility}`);
	});
});
