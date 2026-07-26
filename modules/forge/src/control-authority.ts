import { randomBytes, timingSafeEqual } from "node:crypto";

import { Clock, Context, Deferred, Effect, Layer, Option, Ref } from "effect";

const pair_lifetime_ms = 60_000;
const session_lifetime_ms = 12 * 60 * 60 * 1_000;

export interface ForgeControlAuthorityShape {
	readonly ConsumePair: (code: string) => Effect.Effect<Option.Option<string>>;
	readonly HasSession: (session: string | undefined) => Effect.Effect<boolean>;
	readonly RequestPair: Effect.Effect<string>;
	readonly RequestShutdown: Effect.Effect<void>;
	readonly ShutdownRequested: Effect.Effect<void>;
}

export class ForgeControlAuthority extends Context.Service<
	ForgeControlAuthority,
	ForgeControlAuthorityShape
>()("Artisan/ForgeControlAuthority") {}

export interface ForgeControlAuthorityOptions {
	readonly now?: () => number;
}

/** Creates process-local pairing and shutdown authority for one Forge instance. */
export const make_forge_control_authority_layer = (options: ForgeControlAuthorityOptions = {}) =>
	Layer.effect(
		ForgeControlAuthority,
		Effect.gen(function* () {
			const CurrentTime =
				options.now === undefined ? Clock.currentTimeMillis : Effect.sync(options.now);
			const shutdown = yield* Deferred.make<void>();
			const pairing = yield* Ref.make<
				Option.Option<{ readonly code: string; readonly expires_at: number }>
			>(Option.none());
			const sessions = yield* Ref.make(new Map<string, number>());
			const RequestPair = Effect.gen(function* () {
				const code = randomBytes(32).toString("base64url");
				const now = yield* CurrentTime;
				yield* Ref.set(pairing, Option.some({ code, expires_at: now + pair_lifetime_ms }));
				return code;
			});
			const ConsumePair = (code: string) =>
				Effect.gen(function* () {
					const now = yield* CurrentTime;
					const accepted = yield* Ref.modify(pairing, (current) => {
						if (Option.isNone(current)) return [false, current];
						if (current.value.expires_at < now) {
							return [false, Option.none()];
						}
						if (!same_secret(current.value.code, code)) {
							return [false, current];
						}
						return [true, Option.none()];
					});
					if (!accepted) {
						return Option.none();
					}
					const session = randomBytes(32).toString("base64url");
					yield* Ref.update(sessions, (existing) =>
						new Map(existing).set(session, now + session_lifetime_ms),
					);
					return Option.some(session);
				});

			return ForgeControlAuthority.of({
				ConsumePair,
				HasSession: (session) =>
					CurrentTime.pipe(
						Effect.flatMap((current_time) =>
							Ref.modify(sessions, (existing) => {
								const active = new Map(
									[...existing].filter(
										([, expires_at]) => expires_at >= current_time,
									),
								);
								return [session !== undefined && active.has(session), active];
							}),
						),
					),
				RequestPair,
				RequestShutdown: Deferred.succeed(shutdown, undefined).pipe(Effect.asVoid),
				ShutdownRequested: Deferred.await(shutdown),
			});
		}),
	);

const same_secret = (left: string, right: string) => {
	const left_bytes = Buffer.from(left);
	const right_bytes = Buffer.from(right);
	return left_bytes.length === right_bytes.length && timingSafeEqual(left_bytes, right_bytes);
};
