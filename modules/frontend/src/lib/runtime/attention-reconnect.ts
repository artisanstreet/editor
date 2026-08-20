import { Effect, Layer, Stream } from "effect";
import { ArtisanClient } from "@artisan/transport/client";

import { BrowserReaderAttention } from "../browser/reader-attention";

/**
 * Releases one reconnect authorization the moment the user returns to the
 * window. A hidden or minimized renderer runs on throttled timers, so a
 * transport that dropped there can burn its whole bounded reconnect budget
 * before anyone is watching; the wall-clock gap monitor only fires when the
 * scheduler slept, not when it merely crawled. Attention return is the
 * earliest moment recovery matters again, and RetryConnection is a no-op on
 * a ready connection, so firing on every return is safe.
 *
 * @since 0.2.17
 */
export const AttentionReconnectLive = Layer.effectDiscard(
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const attention = yield* BrowserReaderAttention;

		yield* attention.Changes.pipe(
			Stream.changes,
			/** The first element is the current state, not a return to the window. */
			Stream.drop(1),
			Stream.filter((watching) => watching),
			Stream.runForEach(() =>
				Effect.gen(function* () {
					yield* client.RetryConnection.pipe(Effect.ignore);
				}),
			),
			Effect.forkScoped({ startImmediately: true }),
		);
	}),
);
