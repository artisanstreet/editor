import type { ArtisanConnectionState } from "@artisan/transport/client";
import { ForgeStartLaunchUrl } from "@artisan/protocol";
import { Effect, Match } from "effect";

import { BannerService } from "./service";

export const ForgeConnectionBannerId = "forge-connection";

export const PresentForgeConnectionState = (
	state: ArtisanConnectionState,
	retry_connection: Effect.Effect<void>,
) =>
	Effect.gen(function* () {
		const banner = yield* BannerService;
		const persistent = {
			duration_ms: Number.POSITIVE_INFINITY,
			id: ForgeConnectionBannerId,
		};

		yield* Match.value(state).pipe(
			Match.when({ phase: "connecting" }, () =>
				banner.info("Connecting to Forge…", {
					...persistent,
					description: "Keep this page open while Artisan establishes the session.",
				}),
			),
			Match.when({ phase: "reconnecting" }, () =>
				banner.info("Reconnecting to Forge…", {
					...persistent,
					description: "Your work is safe while Artisan restores the connection.",
				}),
			),
			Match.when({ phase: "exhausted" }, ({ error }) =>
				banner.error("Could not connect to Forge", {
					...persistent,
					actions: [
						{
							href: ForgeStartLaunchUrl,
							icon: "player-play",
							id: "start-forge",
							label: "Start Forge",
						},
						{
							Execute: retry_connection,
							icon: "refresh",
							id: "retry",
							label: "Retry now",
						},
					],
					code: "forge.connection.failed",
					description: "Start the installed local service, or retry this connection.",
					metadata: { transport_message: error.message },
				}),
			),
			Match.when({ phase: "ready" }, () => banner.dismiss(ForgeConnectionBannerId)),
			Match.exhaustive,
		);
	});
