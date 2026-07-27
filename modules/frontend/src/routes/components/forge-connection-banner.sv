<script lang="ts" effect>
	import { Effect, Stream } from "effect";
	import { ArtisanClient } from "@artisan/transport/client";

	import { PresentForgeConnectionState } from "$lib/banner/connection-banner";

	const client = yield* ArtisanClient;
	yield* client.ConnectionState.pipe(
		Effect.flatMap((state) =>
			PresentForgeConnectionState(state, client.RetryConnection),
		),
	);
	yield* client.ConnectionChanges.pipe(
		Stream.runForEach((state) =>
			PresentForgeConnectionState(state, client.RetryConnection),
		),
		Effect.forkScoped,
	);
</script>
