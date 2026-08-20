import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("thread orchestration inspector", () => {
	it("starts directly with context rows instead of a redundant environment heading", () => {
		const card = Read("modules/frontend/src/routes/components/thread-environment-card.svelte");

		expect(card).toContain('<section aria-label="Thread context"');
		expect(card).not.toContain(">Environment<");
		expect(card).not.toContain("thread-environment-heading");
	});

	it("keeps the inspector open for every conversation", () => {
		const layout = Read("modules/frontend/src/routes/+layout.svelte");

		expect(layout).toContain('const thread_inspector_open = $derived(surface === "threads")');
		expect(layout).not.toContain(
			"thread_inspector_fits && (active_checklist !== undefined || active_agents !== undefined)",
		);
		expect(layout).toContain("conversation_plan_has_open_entries(checklist_state.plan)");
	});

	it("puts the environment card above optional thread orchestration", () => {
		const panel = Read("modules/frontend/src/routes/components/thread-panel.svelte");
		const environment = Read(
			"modules/frontend/src/routes/components/thread-environment-card.svelte",
		);

		expect(panel.indexOf("<ThreadEnvironmentCard")).toBeLessThan(
			panel.indexOf(
				'<ShaderGlassSurface class="t-resize t-resize-auto min-h-0 max-h-full shrink rounded-xl">',
			),
		);
		expect(environment).toContain("<ShaderGlassSurface");
		expect(environment).toContain("[--radius-surface:var(--radius-xl)]");
		expect(environment).toContain("Changes");
		expect(environment).toContain("Machine");
		expect(environment).toContain("Branch");
		expect(environment).toContain("Worktree");
		expect(environment.indexOf(">Machine</span>")).toBeLessThan(
			environment.indexOf(">Changes</span>"),
		);
		expect(environment).toContain("{#if change_summary !== undefined}");
		expect(environment).toContain("{#if current_branch !== undefined}");
		expect(environment).toContain("{#if current_worktree_path !== undefined}");
		expect(environment).not.toContain('?? "Unavailable"');
		expect(environment).not.toContain("No branches observed");
		expect(environment).not.toContain("No worktrees observed");
		expect(environment).toContain("yield* HostIdentityController");
		expect(environment).toContain("identity_controller.Refresh.pipe(Effect.forkScoped)");
		expect(environment).toContain("yield* ProjectRepositoryController");
		expect(environment).toContain("yield* GitWorkspaceController");
		expect(environment).toContain("git_workspace_controller.Refresh");
		expect(environment).not.toContain("GetGitWorkspace(");
		expect(environment).toContain("repository_controller.Refresh(next_project_id)");
		expect(environment).not.toContain("GetProjectDiffs(");
		expect(environment).toContain("const change_summary = $derived(workspace?.aggregate)");
		expect(environment).toContain("current_worktree?.path ?? project_root_path");
		for (const icon of ["FileDiff", "DeviceLaptop", "GitBranch", "FolderCode"]) {
			expect(environment).toContain(
				`<${icon} class="size-4 shrink-0 text-muted-foreground" />`,
			);
		}
		expect(environment).toContain("radius-surface");
		expect(environment).toContain("rounded-(--radius-nested)");
		expect(environment).toContain("[--radius-gap:var(--spacing)]");
		expect(environment).not.toContain('<Card size="sm"');
		expect(environment).toContain("RepositoryMarkFor(default_remote.host)");
		expect(environment).toContain("RepositoryDestinationLabel(default_remote.web_url)");
		expect(environment).toContain("card-plastic");
		expect(environment).toContain("{mark.chip}");
		expect(environment).not.toContain("style:background-color");
		expect(environment).toContain('RepositoryChipMarkClass(mark, "size-4")');
		expect(environment).not.toContain('target="_blank"');
		expect(environment).not.toContain("href={default_remote.web_url}");
		expect(environment).toContain("font-mono tabular-nums text-emerald-400");
		expect(environment).toContain('truncate text-foreground">{machine_label}');
		expect(environment).toContain('truncate text-foreground">{BranchLabel(current_branch)}');
		expect(environment).toContain(
			'truncate text-foreground">{WorktreeLabel(current_worktree_path)}',
		);
		expect(environment).toContain("{WorktreeLabel(worktree_path)}");
		expect(environment).toContain('text-xs text-muted-foreground">{worktree_path}');
		expect(environment).toContain('aria-label="Observed branches"');
		expect(environment.trimEnd().endsWith("</section>")).toBe(true);
		expect(panel).toContain('class="relative flex h-full min-h-0 flex-col p-1"');
		expect(panel).not.toContain('class="relative flex h-full min-h-0 flex-col p-4"');
	});

	it("renders active agents and checklist through one shell-owned roster service", () => {
		const layout = Read("modules/frontend/src/routes/+layout.svelte");
		const panel = Read("modules/frontend/src/routes/components/thread-panel.svelte");
		const agents = Read("modules/frontend/src/routes/components/thread-agents.svelte");
		const service = Read("modules/frontend/src/lib/orchestration/service.ts");

		expect(layout).toContain("yield* ThreadOrchestrationRoster");
		expect(layout).toContain("orchestration_lease.Select(active_thread?.thread_id)");
		expect(panel).toContain(
			"<ThreadAgents entries={agents} {hover} oninspect={oninspectagent} {thread_id} />",
		);
		expect(panel).toContain("Checklist");
		expect(
			panel.match(/<ShaderGlassSurface class="t-resize t-resize-auto[^"]*">/gu),
		).toHaveLength(2);
		expect(panel.indexOf("<ThreadAgents")).toBeLessThan(panel.indexOf("Checklist"));
		expect(panel.slice(panel.indexOf("<ThreadAgents"), panel.indexOf("Checklist"))).toContain(
			"</ShaderGlassSurface>",
		);
		expect(agents).toContain('aria-label="Active agents"');
		expect(agents).toContain("{entry.display_name}");
		expect(agents).not.toContain("{entry.role}");
		expect(agents).not.toContain("entry.status");
		expect(agents).not.toContain("WorkingStatusLine");
		expect(agents).toContain("terminal_state(entry.state)");
		expect(agents).toContain('state === "complete" || state === "failed"');
		expect(agents).toContain('state === "failed"');
		expect(service).toContain("client.ListOrchestrationGroups(thread_id, false)");
		expect(service).toContain("client.SubscribeOrchestrationGroups(thread_id, false)");
		expect(service).toContain("client.SubscribeOrchestrationGraph(group.group_id)");
	});

	it("shares one animated hover pill across the environment and agent cards", () => {
		const panel = Read("modules/frontend/src/routes/components/thread-panel.svelte");
		const environment = Read(
			"modules/frontend/src/routes/components/thread-environment-card.svelte",
		);
		const agents = Read("modules/frontend/src/routes/components/thread-agents.svelte");
		const group = Read("modules/frontend/src/routes/components/hover-pill-group.svelte");
		const pill = Read("modules/frontend/src/routes/components/hover-pill.svelte");

		expect(panel).toContain("<HoverPillGroup");
		expect(panel).toContain("{#snippet children({ hover })}");
		for (const card of [environment, agents]) {
			expect(card).toContain("<HoverPill {hover} />");
			expect(card).toContain("onpointerenter={hover.move}");
			expect(card).toContain("onpointermove={hover.move}");
			expect(card).toContain("onfocusin={hover.move}");
			expect(card).not.toContain("hover:bg-");
		}
		expect(panel).toContain("[--docs-sidebar-hover-radius:var(--radius-lg)]");
		expect(group).toContain("observe_hover_target");
		expect(pill).toContain("docs-sidebar-hover-highlight");
		expect(pill).toContain("getBoundingClientRect");
	});

	it("labels each agent with its dispatched model in the model picker's vocabulary", () => {
		const agents = Read("modules/frontend/src/routes/components/thread-agents.svelte");
		const presentation = Read("modules/frontend/src/lib/engine/dispatch-presentation.ts");
		const roster = Read("modules/frontend/src/lib/orchestration/roster.ts");

		expect(agents).toContain("yield* ThreadSessionProjection");
		expect(agents).toContain("session_projection.Changes");
		expect(agents).not.toContain("GetThreadSession");
		expect(agents).toContain(
			"dispatch_model_presentation(policy, entry.engine_id, entry.profile)",
		);
		expect(agents).toContain("{dispatch.model_name}");
		expect(agents).toContain("{dispatch.effort_label}");
		expect(agents).toContain("{dispatch.speed.label}");
		/**
		 * The picker's amber and purple-to-green speed tints belong to its own
		 * trigger. Carried into this card they are the loudest thing in a column
		 * of muted rows, so the row takes the label and leaves the tint.
		 */
		expect(agents).not.toContain("dispatch.speed.class_name");
		/**
		 * One line, identity left and dispatch right, with the dispatch shedding
		 * whole qualifiers as the column narrows rather than truncating them —
		 * a clipped "Extra Hi" would read as a setting nobody chose. Below the
		 * last tier the model becomes its lab's mark.
		 */
		expect(agents).toContain("@container");
		expect(agents).toContain("items-center justify-between gap-4");
		expect(agents).toContain("@min-[13rem]:hidden");
		expect(agents).toContain("@min-[13rem]:inline");
		expect(agents).toContain("@min-[16rem]:inline");
		expect(agents).toContain("@min-[19rem]:inline");
		/** Hidden qualifiers leave the accessibility tree, so the row names them all. */
		expect(agents).toContain("aria-label={dispatch.aria_label}");
		expect(presentation).toContain("aria_label:");
		expect(presentation).toContain("ProviderMarkFor(definition.provider)");
		expect(presentation).toContain("policy.model ?? profile");
		expect(presentation).toContain("speed_option_presentation");
		expect(presentation).toContain("thinking_level_labels");
		expect(roster).toContain("engine_id: assignment.engine_id");
		expect(roster).toContain("profile: assignment.profile");
	});
});
