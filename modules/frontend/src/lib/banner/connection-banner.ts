import type { ArtisanConnectionState } from "@artisan/transport/client";
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
					actions: [
						{
							href: "artisan://forge/start",
							icon: "player-play",
							id: "start-forge",
							label: "Start Forge",
						},
					],
					description:
						"If Forge is not already running, start the installed local service.",
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
							href: "artisan://forge/start",
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
					description: "Five connection attempts failed.",
					metadata: { transport_message: error.message },
				}),
			),
			Match.when({ phase: "ready" }, () => banner.dismiss(ForgeConnectionBannerId)),
			Match.exhaustive,
		);
	});
