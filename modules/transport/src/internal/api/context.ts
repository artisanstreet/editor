import { Context, Effect, Layer } from "effect";

import type { ClientConnectionLifecycle } from "../client-connection";
import type { ClientRequestCoordinator } from "../client-request-coordinator";

export interface ClientApiContextShape {
	readonly MakeTrace: ClientConnectionLifecycle["MakeTrace"];
	readonly MakeId: (prefix: string) => Effect.Effect<string>;
	readonly Request: ClientRequestCoordinator["Request"];
}

interface TransportRuntimeShape {
	readonly MakeId: (prefix: string) => Effect.Effect<string>;
	readonly Now: Effect.Effect<string>;
}

/** Supplies the negotiated request boundary to client operation families. */
export class ClientApiContext extends Context.Service<ClientApiContext, ClientApiContextShape>()(
	"@artisan/transport/internal/ClientApiContext",
) {}

/** Installs one client's request boundary for operation construction. */
export const MakeClientApiContextLayer = (
	connection: ClientConnectionLifecycle,
	requests: ClientRequestCoordinator,
	runtime: TransportRuntimeShape,
) =>
	Layer.succeed(ClientApiContext, {
		MakeTrace: connection.MakeTrace,
		MakeId: runtime.MakeId,
		Request: requests.Request,
	});

/** Constructs one operation family against the client-scoped request boundary. */
export const MakeClientApi = <A, E, R>(
	make: Effect.Effect<A, E, R | ClientApiContext>,
	connection: ClientConnectionLifecycle,
	requests: ClientRequestCoordinator,
	runtime: TransportRuntimeShape,
) => make.pipe(Effect.provide(MakeClientApiContextLayer(connection, requests, runtime)));
