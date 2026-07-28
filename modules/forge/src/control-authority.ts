import { createHash, randomBytes, timingSafeEqual } from "node:crypto";
import { readFile } from "node:fs/promises";

import { Clock, Context, Deferred, Effect, Layer, Option, Ref, Schema, Semaphore } from "effect";

import { WriteFileAtomically } from "./atomic-file";

const pair_lifetime_ms = 60_000;
const session_lifetime_ms = 12 * 60 * 60 * 1_000;

const SessionStore = Schema.Struct({
	sessions: Schema.Array(
		Schema.Struct({
			expires_at: Schema.Int,
			hash: Schema.String.check(Schema.isMinLength(1)),
		}),
	),
	version: Schema.Literal(1),
});

const decode_session_store = Schema.decodeUnknownEffect(SessionStore);

const hash_session = (session: string) => createHash("sha256").update(session).digest("hex");

const LoadSessions = (path: string, now: number) =>
	Effect.tryPromise(() => readFile(path, "utf8")).pipe(
		Effect.flatMap((encoded) => Effect.try(() => JSON.parse(encoded) as unknown)),
		Effect.flatMap((decoded) => decode_session_store(decoded)),
		Effect.map(
			(store) =>
				new Map(
					store.sessions
						.filter((entry) => entry.expires_at >= now)
						.map((entry) => [entry.hash, entry.expires_at] as const),
				),
		),
		Effect.catch(() => Effect.succeed(new Map<string, number>())),
	);

/**
 * Persists session digests atomically. Only SHA-256 digests reach disk:
 * reading the store cannot recover a cookie value, so durability does not
 * widen the trust boundary beyond the profile directory's own permissions.
 */
const PersistSessions = (path: string, sessions: ReadonlyMap<string, number>) =>
	WriteFileAtomically(
		path,
		`${JSON.stringify({
			sessions: [...sessions].map(([hash, expires_at]) => ({ expires_at, hash })),
			version: 1,
		})}\n`,
	).pipe(Effect.ignore);

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
	/**
	 * Durable session digest store. When present, paired sessions survive a
	 * Forge restart instead of logging every browser out on update.
	 */
	readonly session_store_path?: string;
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
			const store_path = options.session_store_path;
			const boot_time = yield* CurrentTime;
			const sessions = yield* Ref.make(
				store_path === undefined
					? new Map<string, number>()
					: yield* LoadSessions(store_path, boot_time),
			);
			/**
			 * Persists are serialized and always re-read the live map, so an
			 * interleaved add and prune can never overwrite the store with a
			 * stale snapshot missing a just-minted session.
			 */
			const persist_gate = yield* Semaphore.make(1);
			const Persist =
				store_path === undefined
					? Effect.void
					: persist_gate.withPermits(1)(
							Ref.get(sessions).pipe(
								Effect.flatMap((current) => PersistSessions(store_path, current)),
							),
						);
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
						new Map(existing).set(hash_session(session), now + session_lifetime_ms),
					);
					yield* Persist;
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
								const allowed =
									session !== undefined && active.has(hash_session(session));
								return [
									{ allowed, pruned: active.size < existing.size },
									active,
								] as const;
							}),
						),
						Effect.flatMap((outcome) =>
							/** Persist only when expiry pruned something, not per request. */
							(outcome.pruned ? Persist : Effect.void).pipe(
								Effect.as(outcome.allowed),
							),
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
