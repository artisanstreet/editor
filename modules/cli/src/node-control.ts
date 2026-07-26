import { Effect, Layer, Schema } from "effect";

import { ForgeControl, ForgeLifecycleError } from "./lifecycle";

const PairResponse = Schema.Struct({
	code: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(256)),
});
const HealthResponse = Schema.Struct({
	instance_id: Schema.String,
	pid: Schema.Int,
	service: Schema.Literal("artisan-forge"),
	status: Schema.Literal("ready"),
	version: Schema.Literal(1),
});

const DecodeLoopbackEndpoint = (endpoint: string) =>
	Effect.try({
		try: () => {
			const url = new URL(endpoint);
			if (
				url.protocol !== "http:" ||
				(url.hostname !== "127.0.0.1" && url.hostname !== "::1") ||
				url.username !== "" ||
				url.password !== "" ||
				url.port === ""
			)
				throw new Error("Forge control requires a loopback endpoint");
			return url.toString();
		},
		catch: (cause) => new ForgeLifecycleError({ cause, code: "ownership" }),
	});

const Request = (endpoint: string, path: string, token: string, method: "GET" | "POST") =>
	DecodeLoopbackEndpoint(endpoint).pipe(
		Effect.flatMap((safe_endpoint) =>
			Effect.tryPromise({
				try: () =>
					fetch(new URL(path, safe_endpoint), {
						headers: { Authorization: `Bearer ${token}` },
						method,
					}),
				catch: (cause) => new ForgeLifecycleError({ cause, code: "timeout" }),
			}),
		),
		Effect.timeout("5 seconds"),
		Effect.mapError((cause) =>
			cause._tag === "ForgeLifecycleError"
				? cause
				: new ForgeLifecycleError({ cause, code: "timeout" }),
		),
	);

/** Authenticated loopback control client; no bearer or pairing material reaches output. */
export const ForgeControlLive = Layer.succeed(
	ForgeControl,
	ForgeControl.of({
		Health: (endpoint, instance_id, token) =>
			Request(endpoint, "/api/control/status", token, "GET").pipe(
				Effect.flatMap((response) =>
					response.ok
						? Effect.tryPromise({
								try: () => response.json(),
								catch: (cause) =>
									new ForgeLifecycleError({ cause, code: "timeout" }),
							})
						: Effect.succeed(undefined),
				),
				Effect.map(
					(body) =>
						body !== undefined &&
						Schema.is(HealthResponse)(body) &&
						body.instance_id === instance_id,
				),
			),
		Shutdown: (endpoint, token) =>
			Request(endpoint, "/api/control/shutdown", token, "POST").pipe(
				Effect.flatMap((response) =>
					response.ok
						? Effect.void
						: Effect.fail(new ForgeLifecycleError({ code: "ownership" })),
				),
			),
		Pair: (endpoint, token) =>
			Request(endpoint, "/api/pair/request", token, "POST").pipe(
				Effect.flatMap((response) =>
					response.ok
						? Effect.tryPromise({
								try: () => response.json(),
								catch: (cause) =>
									new ForgeLifecycleError({ cause, code: "timeout" }),
							})
						: Effect.fail(new ForgeLifecycleError({ code: "ownership" })),
				),
				Effect.flatMap((body) => Schema.decodeUnknownEffect(PairResponse)(body)),
				Effect.map((body) => body.code),
				Effect.mapError((cause) =>
					cause._tag === "ForgeLifecycleError"
						? cause
						: new ForgeLifecycleError({ cause, code: "ownership" }),
				),
			),
	}),
);
