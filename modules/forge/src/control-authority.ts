import { createHash, randomBytes, timingSafeEqual } from "node:crypto";
import { open } from "node:fs/promises";

import { Clock, Context, Deferred, Effect, Layer, Option, Ref, Schema, Semaphore } from "effect";

import { WriteFileAtomically } from "./atomic-file";

const pair_lifetime_ms = 60_000;
const session_lifetime_ms = 12 * 60 * 60 * 1_000;
const session_retention_limit = 256;
const session_store_entry_limit = 4_096;
const session_store_byte_limit = 512 * 1_024;

const SessionStore = Schema.Struct({
	sessions: Schema.Array(
		Schema.Struct({
			expires_at: Schema.Int,
			hash: Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/)),
		}),
	).check(Schema.isMaxLength(session_store_entry_limit)),
	version: Schema.Literal(1),
});

interface SessionExpiry {
	readonly expires_at: number;
	readonly hash: string;
}

interface SessionState {
	readonly closed: boolean;
	readonly expiry: ReadonlyArray<SessionExpiry>;
	readonly generation: number;
	readonly sessions: ReadonlyMap<string, number>;
}

const hash_session = (session: string) => createHash("sha256").update(session).digest("hex");

const ReadSessionStore = (path: string) =>
	Effect.acquireUseRelease(
		Effect.tryPromise(() => open(path, "r")),
		(handle) =>
			Effect.tryPromise(async () => {
				const bytes = Buffer.allocUnsafe(session_store_byte_limit + 1);
				let bytes_read = 0;
				while (bytes_read < bytes.length) {
					const result = await handle.read(
						bytes,
						bytes_read,
						bytes.length - bytes_read,
						bytes_read,
					);
					if (result.bytesRead === 0) break;
					bytes_read += result.bytesRead;
				}
				if (bytes_read > session_store_byte_limit) {
					throw new Error("Forge session store exceeds its byte limit");
				}
				return bytes.toString("utf8", 0, bytes_read);
			}),
		(handle) => Effect.tryPromise(() => handle.close()),
	);

const LoadSessions = (path: string, now: number) =>
	ReadSessionStore(path).pipe(
		Effect.flatMap(Schema.decodeUnknownEffect(Schema.fromJsonString(SessionStore))),
		Effect.map((store) =>
			store.sessions
				.filter((entry) => entry.expires_at >= now)
				.map((entry) => [entry.hash, entry.expires_at] as const),
		),
	);

/** Maintains a min-heap so authorization reads never rebuild the durable map. */
const push_expiry = (entries: ReadonlyArray<SessionExpiry>, entry: SessionExpiry) => {
	const next = [...entries, entry];
	let index = next.length - 1;
	while (index > 0) {
		const parent = Math.floor((index - 1) / 2);
		const parent_entry = next[parent];
		const child_entry = next[index];
		if (parent_entry === undefined || child_entry === undefined) break;
		if (parent_entry.expires_at <= child_entry.expires_at) break;
		next[parent] = child_entry;
		next[index] = parent_entry;
		index = parent;
	}
	return next;
};

const pop_expiry = (entries: ReadonlyArray<SessionExpiry>) => {
	if (entries.length <= 1) return [];
	const next = [...entries];
	const last = next.pop();
	if (last === undefined) return next;
	next[0] = last;
	let index = 0;
	while (true) {
		const left = index * 2 + 1;
		const right = left + 1;
		const current = next[index];
		const left_entry = next[left];
		if (current === undefined || left_entry === undefined) break;
		const right_entry = next[right];
		const child =
			right_entry !== undefined && right_entry.expires_at < left_entry.expires_at
				? right
				: left;
		const child_entry = child === right ? right_entry : left_entry;
		if (child_entry === undefined || current.expires_at <= child_entry.expires_at) break;
		next[index] = child_entry;
		next[child] = current;
		index = child;
	}
	return next;
};

const empty_session_state: SessionState = {
	closed: false,
	expiry: [],
	generation: 0,
	sessions: new Map(),
};

const add_session = (state: SessionState, hash: string, expires_at: number): SessionState => {
	let sessions = new Map(state.sessions).set(hash, expires_at);
	let expiry = push_expiry(
		state.sessions.has(hash)
			? state.expiry.filter((entry) => entry.hash !== hash)
			: state.expiry,
		{ expires_at, hash },
	);
	while (sessions.size > session_retention_limit) {
		const oldest = expiry[0];
		if (oldest === undefined) break;
		expiry = pop_expiry(expiry);
		if (sessions.get(oldest.hash) === oldest.expires_at) sessions.delete(oldest.hash);
	}
	return { ...state, expiry, generation: state.generation + 1, sessions };
};

const prune_expired = (state: SessionState, now: number) => {
	let expiry = state.expiry;
	let pruned: Map<string, number> | undefined;
	while (true) {
		const oldest = expiry[0];
		if (oldest === undefined || oldest.expires_at >= now) break;
		expiry = pop_expiry(expiry);
		if ((pruned ?? state.sessions).get(oldest.hash) === oldest.expires_at) {
			pruned ??= new Map(state.sessions);
			pruned.delete(oldest.hash);
		}
	}
	return pruned === undefined
		? { state, pruned: false }
		: { state: { ...state, expiry, sessions: pruned }, pruned: true };
};

const restore_sessions = (entries: ReadonlyArray<readonly [string, number]>) => {
	const by_digest = new Map<string, number>();
	for (const [hash, expires_at] of entries) {
		const existing = by_digest.get(hash);
		if (existing === undefined || existing < expires_at) by_digest.set(hash, expires_at);
	}
	return [...by_digest]
		.sort((left, right) => right[1] - left[1])
		.slice(0, session_retention_limit)
		.reduce(
			(state, [hash, expires_at]) => add_session(state, hash, expires_at),
			empty_session_state,
		);
};

/**
 * Persists session digests atomically. Only SHA-256 digests reach disk:
 * reading the store cannot recover a cookie value, so durability does not
 * widen the trust boundary beyond the home directory's own permissions.
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
	/** Durable session digest store. Paired sessions survive a Forge restart. */
	readonly session_store_path?: string;
	/** Test seam for holding or failing durable startup without delaying bind. */
	readonly load_sessions?: (
		path: string,
		now: number,
	) => Effect.Effect<ReadonlyArray<readonly [string, number]>, unknown>;
	/** Test seam for proving mint, close, and durable publication ordering. */
	readonly persist_sessions?: (
		path: string,
		sessions: ReadonlyMap<string, number>,
	) => Effect.Effect<void>;
}

/** Creates process-local pairing and shutdown authority for one Forge instance. */
export const make_forge_control_authority_layer = (options: ForgeControlAuthorityOptions = {}) =>
	Layer.effect(
		ForgeControlAuthority,
		Effect.gen(function* () {
			const CurrentTime =
				options.now === undefined ? Clock.currentTimeMillis : Effect.sync(options.now);
			const shutdown = yield* Deferred.make<void>();
			const initial_load = yield* Deferred.make<void>();
			const pairing = yield* Ref.make<
				Option.Option<{ readonly code: string; readonly expires_at: number }>
			>(Option.none());
			const sessions = yield* Ref.make(empty_session_state);
			const lifecycle_gate = yield* Semaphore.make(1);
			const persist_gate = yield* Semaphore.make(1);
			const store_path = options.session_store_path;
			const persist_sessions = options.persist_sessions ?? PersistSessions;
			const Persist =
				store_path === undefined
					? Effect.void
					: persist_gate.withPermits(1)(
							Ref.get(sessions).pipe(
								Effect.flatMap((current) =>
									current.closed
										? Effect.void
										: persist_sessions(store_path, current.sessions),
								),
							),
						);

			yield* Effect.addFinalizer(() =>
				lifecycle_gate.withPermits(1)(
					Ref.update(sessions, (current) => ({ ...current, closed: true })).pipe(
						Effect.andThen(Deferred.interrupt(initial_load)),
					),
				),
			);
			if (store_path === undefined) {
				yield* Deferred.succeed(initial_load, undefined);
			} else {
				const boot_time = yield* CurrentTime;
				const load_generation = (yield* Ref.get(sessions)).generation;
				const load_sessions = options.load_sessions ?? LoadSessions;
				yield* load_sessions(store_path, boot_time).pipe(
					Effect.map(restore_sessions),
					Effect.flatMap((loaded) =>
						Ref.update(sessions, (current) => {
							if (current.closed) return current;
							if (current.generation === load_generation) {
								return { ...loaded, generation: current.generation + 1 };
							}
							/** Local sessions win any load/persist interleaving. */
							return [...loaded.sessions].reduce(
								(next, [hash, expires_at]) =>
									current.sessions.has(hash)
										? next
										: add_session(next, hash, expires_at),
								current,
							);
						}),
					),
					Effect.andThen(Persist),
					Effect.catch(() => Effect.void),
					Effect.ensuring(Deferred.succeed(initial_load, undefined).pipe(Effect.asVoid)),
					Effect.forkScoped({ startImmediately: true }),
				);
			}

			const RequestPair = lifecycle_gate.withPermits(1)(
				Effect.gen(function* () {
					if ((yield* Ref.get(sessions)).closed) return yield* Effect.interrupt;
					const code = randomBytes(32).toString("base64url");
					const now = yield* CurrentTime;
					yield* Ref.set(
						pairing,
						Option.some({ code, expires_at: now + pair_lifetime_ms }),
					);
					return code;
				}),
			);
			const ConsumePair = (code: string) =>
				Effect.gen(function* () {
					yield* Deferred.await(initial_load);
					const minted = yield* lifecycle_gate.withPermits(1)(
						Effect.gen(function* () {
							if ((yield* Ref.get(sessions)).closed) return yield* Effect.interrupt;
							const now = yield* CurrentTime;
							const accepted = yield* Ref.modify(pairing, (current) => {
								if (Option.isNone(current)) return [false, current];
								if (current.value.expires_at < now) return [false, Option.none()];
								if (!same_secret(current.value.code, code)) return [false, current];
								return [true, Option.none()];
							});
							if (!accepted) return Option.none<string>();
							const session = randomBytes(32).toString("base64url");
							yield* Ref.update(sessions, (current) =>
								add_session(
									current,
									hash_session(session),
									now + session_lifetime_ms,
								),
							);
							return Option.some(session);
						}),
					);
					if (Option.isNone(minted)) return minted;
					yield* Persist;
					return yield* lifecycle_gate.withPermits(1)(
						Ref.get(sessions).pipe(
							Effect.map((current) => (current.closed ? Option.none() : minted)),
						),
					);
				});

			return ForgeControlAuthority.of({
				ConsumePair,
				HasSession: (session) =>
					Deferred.await(initial_load).pipe(
						Effect.andThen(CurrentTime),
						Effect.flatMap((current_time) =>
							Ref.modify(sessions, (current) => {
								if (current.closed) {
									return [{ allowed: false, pruned: false }, current] as const;
								}
								const outcome = prune_expired(current, current_time);
								return [
									{
										allowed:
											session !== undefined &&
											outcome.state.sessions.has(hash_session(session)),
										pruned: outcome.pruned,
									},
									outcome.state,
								] as const;
							}),
						),
						Effect.flatMap((outcome) =>
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
