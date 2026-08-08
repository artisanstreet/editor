import { Effect, Option, Ref } from "effect";

import {
	SubscriptionContext,
	SubscriptionErrorReporter,
	SubscriptionIdentity,
	SubscriptionOptions,
	SubscriptionProtocol,
} from "./context";
import type { SubscriptionState } from "./model";
import { MakeClientSubscriptionCoordinator } from "./registry";

export const make_client_subscription_coordinator = Effect.gen(function* () {
	const { event_capacity, subscription_capacity } = yield* SubscriptionOptions;
	const { make_id, make_trace } = yield* SubscriptionIdentity;
	const { send_current } = yield* SubscriptionProtocol;
	const { publish_error } = yield* SubscriptionErrorReporter;
	const state = yield* Ref.make<SubscriptionState>({
		disposed: false,
		event_observers: new Map(),
		event_cursors: {},
		event_terminal: { _tag: "active" },
		ignored_correlations: new Set(),
		last_journal_sequence: 0,
		session_send: Option.none(),
		subscriptions: new Map(),
	});
	return yield* MakeClientSubscriptionCoordinator.pipe(
		Effect.provideService(SubscriptionContext, {
			event_capacity,
			make_id,
			make_trace,
			publish_error,
			send_current,
			state,
			subscription_capacity,
		}),
	);
});
