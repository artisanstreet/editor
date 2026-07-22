<script lang="ts" effect>
	import { Effect, Stream } from "effect";

	import { LiveWorkspaceStore, type LiveWorkspaceSnapshot } from "$lib/live-workspace/store";
	import WelcomePage from "./components/welcome-page.sv";

	const live_workspace = yield* LiveWorkspaceStore;
	let live_snapshot = $state.raw<LiveWorkspaceSnapshot>(yield* live_workspace.Snapshot);

	yield* Stream.runForEach(live_workspace.Changes, (next_snapshot) => {
		live_snapshot = next_snapshot;
	}).pipe(Effect.forkScoped);
</script>

<WelcomePage {live_snapshot} />
