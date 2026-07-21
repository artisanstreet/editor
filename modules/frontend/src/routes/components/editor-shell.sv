<script lang="ts" effect>
	import { Effect, Stream } from "effect";
	import type { DesktopIdentity } from "@artisan/transport/client";
	import { IconLayoutSidebar as PanelLeft, IconLayoutSidebarRight as PanelRight } from "@tabler/icons-svelte";
	import {
		DefaultShellPresentationState,
		ShellPresentationPreferences,
	} from "$lib/runtime/shell-presentation-preferences";
	import { Button } from "$lib/components/ui/button";
	import { Sheet, SheetContent, SheetTrigger } from "$lib/components/ui/sheet";
	import { HasActiveWorkspaceWork } from "$lib/live-workspace/activity-status";
	import { LiveWorkspaceStore, type LiveWorkspaceSnapshot } from "$lib/live-workspace/store";

	import LeftPane from "./left-pane.sv";
	import MainPane from "./main-pane.sv";
	import RightPane from "./right-pane.sv";

	const shell_presentation_preferences = yield* ShellPresentationPreferences;
	const live_workspace = yield* LiveWorkspaceStore;
	const fallback_identity: DesktopIdentity = {
		display_name: "Local user",
		machine_name: "Local machine",
		avatar_seed: "artisan:local",
	};
	let desktop_identity = $state.raw<DesktopIdentity>(fallback_identity);
	const desktop_bridge = typeof window === "undefined" ? undefined : window.artisanDesktop;
	if (desktop_bridge !== undefined) {
		desktop_identity = yield* Effect.tryPromise(() => desktop_bridge.identity()).pipe(
			Effect.catch(() => Effect.succeed(fallback_identity)),
		);
	}
	const initial_presentation = yield* shell_presentation_preferences.Load;
	let live_snapshot = $state.raw<LiveWorkspaceSnapshot>(yield* live_workspace.Snapshot);
	let desktop_working = false;
	const SetDesktopWorking = (working: boolean) =>
		Effect.suspend(() => {
			if (desktop_bridge === undefined || desktop_working === working) return Effect.void;
			const previous = desktop_working;
			desktop_working = working;
			return Effect.tryPromise(async () => {
				await desktop_bridge.setWorking(working);
			}).pipe(
				Effect.tapError(() =>
					Effect.sync(() => {
						if (desktop_working === working) desktop_working = previous;
					}),
				),
				Effect.ignore,
			);
		});
	yield* SetDesktopWorking(HasActiveWorkspaceWork(live_snapshot));
	yield* Effect.addFinalizer(SetDesktopWorking(false));
	yield* Stream.runForEach(live_workspace.Changes, (next_snapshot) =>
		Effect.gen(function* () {
			live_snapshot = next_snapshot;
			yield* SetDesktopWorking(HasActiveWorkspaceWork(next_snapshot));
		}),
	).pipe(Effect.forkScoped);

	let left_open = $state(false);
	let right_open = $state(false);
	let left_collapsed = $state(initial_presentation.left_collapsed);
	let right_collapsed = $state(initial_presentation.right_collapsed);

	const SavePresentation = Effect.gen(function* () {
		yield* shell_presentation_preferences.Save({
			...DefaultShellPresentationState,
			left_collapsed,
			right_collapsed,
		});
	});

	const ExpandLeft = Effect.gen(function* () {
		left_collapsed = false;
		yield* SavePresentation;
	});

	const ExpandRight = Effect.gen(function* () {
		right_collapsed = false;
		yield* SavePresentation;
	});

	const CollapseLeft = Effect.gen(function* () {
		left_collapsed = true;
		left_open = false;
		yield* SavePresentation;
	});

	const CollapseRight = Effect.gen(function* () {
		right_collapsed = true;
		right_open = false;
		yield* SavePresentation;
	});

	const ClosePanes = Effect.gen(function* () {
		left_open = false;
		right_open = false;
	});

	const SelectThread = (thread_id: string) =>
		Effect.gen(function* () {
			yield* live_workspace.SelectThread(thread_id);
		});

	const NewChat = Effect.gen(function* () {
		yield* live_workspace.CreateThread("New chat");
	});

	const HandleKeydown = (pressed_key: string) =>
		Effect.gen(function* () {
			if (pressed_key === "Escape") {
				yield* ClosePanes;
			}
		});
</script>

<svelte:window onkeydown={yield* HandleKeydown(event.key)} />

<div
	class="editor-shell"
	data-left-collapsed={left_collapsed}
	data-left-open={left_open}
	data-right-collapsed={right_collapsed}
	data-right-open={right_open}
>
	<div class="pane-slot desktop-left-slot">
		<LeftPane compact={false} instance_id="desktop-left" live_snapshot={live_snapshot} identity={desktop_identity} actions={live_workspace.Actions} marketplace_api={live_workspace} on_select_thread={SelectThread} on_new_chat={NewChat} on_collapse={CollapseLeft} />
	</div>

	<div class="left-rail-slot">
		<LeftPane compact={true} instance_id="rail-left" live_snapshot={live_snapshot} identity={desktop_identity} actions={live_workspace.Actions} marketplace_api={live_workspace} on_select_thread={SelectThread} on_new_chat={NewChat} />
	</div>

	<main class="main-slot">
		<div class="compact-pane-actions" aria-label="Open workspace panes">
			<Button variant="outline" size="icon-sm" class="desktop-left-toggle size-8" aria-label="Expand thread navigation" onclick={yield* ExpandLeft}>
				<PanelLeft size={18} stroke={1.7} aria-hidden="true" />
			</Button>
			<Button variant="outline" size="icon-sm" class="desktop-right-toggle size-8" aria-label="Expand session pane" onclick={yield* ExpandRight}>
				<PanelRight size={18} stroke={1.7} aria-hidden="true" />
			</Button>
			<Sheet bind:open={left_open}>
				<SheetTrigger>
					{#snippet child({ props })}
						<Button variant="outline" size="icon-sm" class="responsive-left-toggle size-8" aria-label="Open thread navigation" {...props}>
							<PanelLeft size={18} stroke={1.7} aria-hidden="true" />
						</Button>
					{/snippet}
				</SheetTrigger>
				<SheetContent side="left" class="w-[min(18rem,calc(100vw-1.5rem))] p-0" aria-label="Thread navigation">
					<LeftPane compact={false} instance_id="sheet-left" live_snapshot={live_snapshot} identity={desktop_identity} actions={live_workspace.Actions} marketplace_api={live_workspace} on_select_thread={SelectThread} on_new_chat={NewChat} />
				</SheetContent>
			</Sheet>
			<Sheet bind:open={right_open}>
				<SheetTrigger>
					{#snippet child({ props })}
						<Button variant="outline" size="icon-sm" class="responsive-right-toggle size-8" aria-label="Open session pane" {...props}>
							<PanelRight size={18} stroke={1.7} aria-hidden="true" />
						</Button>
					{/snippet}
				</SheetTrigger>
				<SheetContent side="right" class="w-[min(21.25rem,calc(100vw-1.5rem))] p-0" aria-label="Session">
					<RightPane instance_id="sheet-right" live_snapshot={live_snapshot} controller={live_workspace} />
				</SheetContent>
			</Sheet>
		</div>
		<MainPane
			live_snapshot={live_snapshot}
			actions={live_workspace.Actions}
			on_send_live_message={live_workspace.SendMessage}
			on_refresh_workspace_files={live_workspace.RefreshWorkspaceFiles}
			on_read_workspace_file={live_workspace.ReadWorkspaceFile}
			on_replace_workspace_file={live_workspace.ReplaceWorkspaceFile}
			on_select_orchestration_group={live_workspace.SelectOrchestrationGroup}
		/>
	</main>

	<div class="pane-slot desktop-right-slot">
		<RightPane instance_id="desktop-right" live_snapshot={live_snapshot} controller={live_workspace} on_collapse={CollapseRight} />
	</div>

</div>

<style>
	.editor-shell {
		--pane-action-space: 10px;

		display: grid;
		grid-template-columns: 272px minmax(720px, 1fr) 340px;
		gap: 12px;
		height: 100dvh;
		min-height: 0;
		padding: 8px;
		background: var(--canvas);
		color: var(--text-primary);
		overflow: hidden;
	}

	.pane-slot,
	.main-slot,
	.left-rail-slot {
		min-width: 0;
		min-height: 0;
	}

	.pane-slot,
	.main-slot {
		position: relative;
		z-index: 1;
	}

	.main-slot {
		overflow: hidden;
	}

	.left-rail-slot,
	.compact-pane-actions,
	:global(.desktop-left-toggle),
	:global(.desktop-right-toggle),
	:global(.responsive-left-toggle),
	:global(.responsive-right-toggle) {
		display: none;
	}

	.editor-shell[data-left-collapsed="true"] {
		grid-template-columns: 56px minmax(720px, 1fr) 340px;
		--pane-action-space: 48px;
	}

	.editor-shell[data-right-collapsed="true"] {
		grid-template-columns: 272px minmax(720px, 1fr);
		--pane-action-space: 48px;
	}

	.editor-shell[data-left-collapsed="true"][data-right-collapsed="true"] {
		grid-template-columns: 56px minmax(720px, 1fr);
		--pane-action-space: 82px;
	}

	.editor-shell[data-left-collapsed="true"] .desktop-left-slot,
	.editor-shell[data-right-collapsed="true"] .desktop-right-slot {
		display: none;
	}

	.editor-shell[data-left-collapsed="true"] .left-rail-slot,
	.editor-shell[data-left-collapsed="true"] .compact-pane-actions,
	.editor-shell[data-left-collapsed="true"] :global(.desktop-left-toggle),
	.editor-shell[data-right-collapsed="true"] .compact-pane-actions,
	.editor-shell[data-right-collapsed="true"] :global(.desktop-right-toggle) {
		display: grid;
	}

	.editor-shell[data-left-collapsed="true"] .compact-pane-actions,
	.editor-shell[data-right-collapsed="true"] .compact-pane-actions {
		position: absolute;
		top: 10px;
		right: 10px;
		z-index: 10;
		display: flex;
		gap: 6px;
	}

	@media (min-width: 1280px) and (max-width: 1367px) {
		.editor-shell {
			grid-template-columns: 240px minmax(720px, 1fr) 280px;
		}

		.editor-shell[data-left-collapsed="true"] {
			grid-template-columns: 56px minmax(720px, 1fr) 280px;
		}

		.editor-shell[data-right-collapsed="true"] {
			grid-template-columns: 240px minmax(720px, 1fr);
		}

		.editor-shell[data-left-collapsed="true"][data-right-collapsed="true"] {
			grid-template-columns: 56px minmax(720px, 1fr);
		}
	}

	@media (max-width: 1279px) {
		.editor-shell {
			--pane-action-space: 48px;

			grid-template-columns: 240px minmax(0, 1fr);
		}

		.editor-shell[data-left-collapsed="true"],
		.editor-shell[data-right-collapsed="true"],
		.editor-shell[data-left-collapsed="true"][data-right-collapsed="true"] {
			grid-template-columns: 240px minmax(0, 1fr);
		}

		.editor-shell .desktop-right-slot {
			display: none;
		}

		.editor-shell[data-left-collapsed="true"] .desktop-left-slot {
			display: block;
		}

		.editor-shell[data-left-collapsed="true"] .left-rail-slot,
		.editor-shell[data-left-collapsed="true"] :global(.desktop-left-toggle),
		.editor-shell[data-right-collapsed="true"] :global(.desktop-right-toggle) {
			display: none;
		}

		:global(.responsive-right-toggle) {
			display: grid;
		}

		.compact-pane-actions {
			position: absolute;
			top: 10px;
			right: 10px;
			z-index: 10;
			display: flex;
		}

	}

	@media (min-width: 800px) and (max-width: 999px) {
		.editor-shell,
		.editor-shell[data-left-collapsed="true"],
		.editor-shell[data-right-collapsed="true"],
		.editor-shell[data-left-collapsed="true"][data-right-collapsed="true"] {
			grid-template-columns: 56px minmax(0, 1fr);
		}

		.editor-shell .desktop-left-slot,
		.editor-shell[data-left-collapsed="true"] .desktop-left-slot {
			display: none;
		}

		.editor-shell .left-rail-slot,
		.editor-shell[data-left-collapsed="true"] .left-rail-slot {
			display: block;
		}
	}

	@media (max-width: 799px) {
		.editor-shell,
		.editor-shell[data-left-collapsed="true"],
		.editor-shell[data-right-collapsed="true"],
		.editor-shell[data-left-collapsed="true"][data-right-collapsed="true"] {
			--pane-action-space: 82px;

			grid-template-columns: minmax(0, 1fr);
			gap: 0;
			padding: 6px;
		}

		.editor-shell .desktop-left-slot,
		.editor-shell[data-left-collapsed="true"] .desktop-left-slot,
		.editor-shell .left-rail-slot,
		.editor-shell[data-left-collapsed="true"] .left-rail-slot {
			display: none;
		}

		:global(.responsive-left-toggle),
		:global(.responsive-right-toggle) {
			display: grid;
		}

		.compact-pane-actions {
			position: absolute;
			top: 8px;
			right: 8px;
			z-index: 10;
			display: flex;
			gap: 6px;
		}
	}

</style>
