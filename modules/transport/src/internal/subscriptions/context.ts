import { Context, Effect, Ref } from "effect";

import type { ArtisanClientError } from "../../client-api/service";
import type { MakeTrace, SendCurrent } from "../client-common";
import type { SubscriptionState } from "./model";

export interface SubscriptionOptionsShape {
	readonly event_capacity: number;
	readonly subscription_capacity: number;
}

/** Bounded queue configuration for one subscription coordinator. */
export class SubscriptionOptions extends Context.Service<
	SubscriptionOptions,
	SubscriptionOptionsShape
>()("@artisan/transport/internal/SubscriptionOptions") {}

export interface SubscriptionIdentityShape {
	readonly make_id: (prefix: string) => Effect.Effect<string>;
	readonly make_trace: MakeTrace;
}

/** Identity and tracing capabilities used when constructing protocol envelopes. */
export class SubscriptionIdentity extends Context.Service<
	SubscriptionIdentity,
	SubscriptionIdentityShape
>()("@artisan/transport/internal/SubscriptionIdentity") {}

export interface SubscriptionProtocolShape {
	readonly send_current: SendCurrent;
}

/** Active-session protocol delivery capability. */
export class SubscriptionProtocol extends Context.Service<
	SubscriptionProtocol,
	SubscriptionProtocolShape
>()("@artisan/transport/internal/SubscriptionProtocol") {}

export interface SubscriptionErrorReporterShape {
	readonly publish_error: (error: ArtisanClientError) => Effect.Effect<void>;
}

/** Reports asynchronous subscription delivery failures to the owning client. */
export class SubscriptionErrorReporter extends Context.Service<
	SubscriptionErrorReporter,
	SubscriptionErrorReporterShape
>()("@artisan/transport/internal/SubscriptionErrorReporter") {}

export interface SubscriptionContextShape {
	readonly event_capacity: number;
	readonly make_id: (prefix: string) => Effect.Effect<string>;
	readonly make_trace: MakeTrace;
	readonly publish_error: (error: ArtisanClientError) => Effect.Effect<void>;
	readonly send_current: SendCurrent;
	readonly state: Ref.Ref<SubscriptionState>;
	readonly subscription_capacity: number;
}

/** Supplies one connection's capabilities and mutable subscription state. */
export class SubscriptionContext extends Context.Service<
	SubscriptionContext,
	SubscriptionContextShape
>()("@artisan/transport/internal/SubscriptionContext") {}
