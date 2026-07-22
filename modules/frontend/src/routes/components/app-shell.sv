<script lang="ts" effect>
	import { Effect, Stream } from "effect";
	import type { DesktopIdentity } from "@artisan/transport/client";

	import { HasActiveWorkspaceWork } from "$lib/live-workspace/activity-status";
	import { LiveWorkspaceStore, type LiveWorkspaceSnapshot } from "$lib/live-workspace/store";
	import AppSidebar from "./app-sidebar.sv";

	let { children } = $props();

	const live_workspace = yield* LiveWorkspaceStore;
	const fallback_identity: DesktopIdentity = {
		display_name: "Local user",
		machine_name: "Local machine",
		avatar_seed: "artisan:local",
	};
	const desktop_bridge = typeof window === "undefined" ? undefined : window.artisanDesktop;
	let desktop_identity = $state.raw<DesktopIdentity>(fallback_identity);
	let desktop_working = false;
	let live_snapshot = $state.raw<LiveWorkspaceSnapshot>(yield* live_workspace.Snapshot);

	if (desktop_bridge !== undefined) {
		desktop_identity = yield* Effect.tryPromise(() => desktop_bridge.identity()).pipe(
			Effect.catch(() => Effect.succeed(fallback_identity)),
		);
	}

	const SetDesktopWorking = (working: boolean) =>
		Effect.suspend(() => {
			if (desktop_bridge === undefined || desktop_working === working) return Effect.void;
			desktop_working = working;
			return Effect.tryPromise(() => desktop_bridge.setWorking(working)).pipe(Effect.ignore);
		});

	yield* SetDesktopWorking(HasActiveWorkspaceWork(live_snapshot));
	yield* Effect.addFinalizer(SetDesktopWorking(false));
	yield* Stream.runForEach(live_workspace.Changes, (next_snapshot) =>
		Effect.gen(function* () {
			live_snapshot = next_snapshot;
			yield* SetDesktopWorking(HasActiveWorkspaceWork(next_snapshot));
		}),
	).pipe(Effect.forkScoped);
</script>

<div class="app-shell">
	<AppSidebar {live_snapshot} identity={desktop_identity} on_create_thread={live_workspace.CreateThread} />
	<main class="app-content">{@render children()}</main>
</div>

<style>
	.app-shell {
		display: grid;
		grid-template-columns: 15.5rem minmax(0, 1fr);
		gap: 0.5rem;
		height: 100dvh;
		min-height: 0;
		padding: 0.5rem;
		background: var(--canvas);
		color: var(--text-primary);
		overflow: hidden;
	}

	.app-content {
		min-width: 0;
		min-height: 0;
		border: 1px solid var(--line);
		border-radius: 1.5rem;
		background: var(--pane);
		overflow: auto;
	}

	@media (max-width: 760px) {
		.app-shell {
			grid-template-columns: minmax(0, 1fr);
			padding: 0.375rem;
		}

		.app-content {
			border-radius: 1.25rem;
		}
	}
</style>
