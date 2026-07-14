import { Clock, Effect, Layer, Option, PubSub, Schema, Stream } from "effect";

import { CommandEnvelope, type CommandEnvelope as Command } from "@artisan/protocol";

import { is_local_preview_hostname } from "./network-policy";
import {
	PreviewHealthProbe,
	PreviewTarget,
	PreviewTargetClock,
	PreviewTargetError,
	type PreviewTargetAcceptance,
} from "./preview-target";
import {
	PreviewTargetRepository,
	PreviewTargetRepositoryConflict,
	PreviewTargetRepositoryInvariant,
	PreviewTargetRepositoryMissing,
	PreviewTargetRepositoryStorage,
	PreviewTargetRepositoryUnavailable,
	type PreviewTargetProbeClaimResult,
} from "./preview-target-repository";

/** Configures the bounded process-local feed of committed preview events. */
export interface PreviewTargetOptions {
	readonly probe_lease_ms?: number;
	readonly probe_poll_interval_ms?: number;
	readonly probe_timeout_ms?: number;
	readonly sliding_event_capacity?: number;
}

type PreviewCommand = Extract<Command["payload"], { readonly type: `preview.target.${string}` }>;
type ReadyProbeClaim = Exclude<PreviewTargetProbeClaimResult, { readonly _tag: "Pending" }>;

function is_register_command(command: Command): command is Command & {
	readonly payload: Extract<PreviewCommand, { readonly type: "preview.target.register" }>;
} {
	return command.payload.type === "preview.target.register";
}

function is_probe_command(command: Command): command is Command & {
	readonly payload: Extract<PreviewCommand, { readonly type: "preview.target.probe" }>;
} {
	return command.payload.type === "preview.target.probe";
}

function is_remove_command(command: Command): command is Command & {
	readonly payload: Extract<PreviewCommand, { readonly type: "preview.target.remove" }>;
} {
	return command.payload.type === "preview.target.remove";
}

function target_error(target_id: string, code: PreviewTargetError["code"]): PreviewTargetError {
	return new PreviewTargetError({ code, target_id });
}

function parse_local_preview_url(command: Command) {
	return Effect.gen(function* () {
		const decoded = yield* Schema.decodeUnknownEffect(CommandEnvelope, {
			onExcessProperty: "error",
		})(command).pipe(Effect.mapError(() => target_error("", "invalid_target")));

		if (!is_register_command(decoded)) {
			return yield* Effect.fail(target_error("", "invalid_target"));
		}

		const url = yield* Effect.try({
			try: () => new URL(decoded.payload.url),
			catch: () => target_error(decoded.payload.target_id, "invalid_target"),
		});

		if (
			(url.protocol !== "http:" && url.protocol !== "https:") ||
			url.username ||
			url.password ||
			!is_local_preview_hostname(url.hostname)
		) {
			return yield* Effect.fail(target_error(decoded.payload.target_id, "invalid_target"));
		}

		return url.href;
	});
}

function map_repository_error(target_id: string, error: unknown): PreviewTargetError {
	if (error instanceof PreviewTargetRepositoryMissing) {
		return target_error(target_id, "not_found");
	}

	if (error instanceof PreviewTargetRepositoryConflict) {
		return target_error(target_id, "conflict");
	}

	if (error instanceof PreviewTargetRepositoryInvariant) {
		return target_error(target_id, "invariant");
	}

	if (error instanceof PreviewTargetRepositoryStorage) {
		return target_error(target_id, "unavailable");
	}

	if (error instanceof PreviewTargetRepositoryUnavailable) {
		return target_error(target_id, "unavailable");
	}

	return target_error(target_id, "invariant");
}

/** Builds the durable preview target service with a scoped replaceable probe seam. */
export function make_preview_target_layer(options: PreviewTargetOptions = {}) {
	const probe_lease_ms = options.probe_lease_ms ?? 30_000;
	const probe_poll_interval_ms = options.probe_poll_interval_ms ?? 25;
	const probe_timeout_ms = options.probe_timeout_ms ?? 20_000;
	const sliding_event_capacity = options.sliding_event_capacity ?? 128;

	return Layer.effect(
		PreviewTarget,
		Effect.gen(function* () {
			if (
				!Number.isSafeInteger(sliding_event_capacity) ||
				sliding_event_capacity <= 0 ||
				!Number.isSafeInteger(probe_lease_ms) ||
				probe_lease_ms <= 0 ||
				probe_lease_ms > 600_000 ||
				!Number.isSafeInteger(probe_timeout_ms) ||
				probe_timeout_ms <= 0 ||
				probe_timeout_ms >= probe_lease_ms ||
				!Number.isSafeInteger(probe_poll_interval_ms) ||
				probe_poll_interval_ms <= 0 ||
				probe_poll_interval_ms >= probe_lease_ms
			) {
				return yield* Effect.fail(target_error("", "invalid_target"));
			}

			const clock = yield* PreviewTargetClock;
			const health_probe = yield* PreviewHealthProbe;
			const repository = yield* PreviewTargetRepository;
			const events = yield* Effect.acquireRelease(
				PubSub.sliding<PreviewTargetAcceptance["event"]>(sliding_event_capacity),
				PubSub.shutdown,
			);
			const publish = (acceptance: PreviewTargetAcceptance) =>
				acceptance.status === "accepted"
					? PubSub.publish(events, acceptance.event).pipe(Effect.asVoid)
					: Effect.void;
			const AwaitProbeClaim = (
				command: Command & {
					readonly payload: Extract<
						PreviewCommand,
						{ readonly type: "preview.target.probe" }
					>;
				},
			): Effect.Effect<ReadyProbeClaim, PreviewTargetError> =>
				repository.ClaimProbe(command, probe_lease_ms).pipe(
					Effect.mapError((error) =>
						map_repository_error(command.payload.target_id, error),
					),
					Effect.flatMap((result) =>
						result._tag === "Pending"
							? Effect.sleep(probe_poll_interval_ms).pipe(
									Effect.andThen(Effect.suspend(() => AwaitProbeClaim(command))),
								)
							: Effect.succeed(result),
					),
				);
			const Register = (command: Command) =>
				Effect.gen(function* () {
					const decoded = yield* Schema.decodeUnknownEffect(CommandEnvelope)(
						command,
					).pipe(Effect.mapError(() => target_error("", "invalid_target")));

					if (!is_register_command(decoded)) {
						return yield* Effect.fail(target_error("", "invalid_target"));
					}

					const replayed = yield* repository
						.Replay(decoded)
						.pipe(
							Effect.mapError((error) =>
								map_repository_error(decoded.payload.target_id, error),
							),
						);

					if (Option.isSome(replayed)) {
						return replayed.value;
					}

					const url = yield* parse_local_preview_url(decoded);
					const now_ms = yield* clock.Now;
					const acceptance = yield* repository
						.Register(decoded, url, now_ms)
						.pipe(
							Effect.mapError((error) =>
								map_repository_error(decoded.payload.target_id, error),
							),
						);

					yield* publish(acceptance);

					return acceptance;
				});
			const Remove = (command: Command) =>
				Effect.gen(function* () {
					const decoded = yield* Schema.decodeUnknownEffect(CommandEnvelope)(
						command,
					).pipe(Effect.mapError(() => target_error("", "invalid_target")));

					if (!is_remove_command(decoded)) {
						return yield* Effect.fail(target_error("", "invalid_target"));
					}

					const replayed = yield* repository
						.Replay(decoded)
						.pipe(
							Effect.mapError((error) =>
								map_repository_error(decoded.payload.target_id, error),
							),
						);

					if (Option.isSome(replayed)) {
						return replayed.value;
					}

					const now_ms = yield* clock.Now;
					const acceptance = yield* repository
						.Remove(decoded, now_ms)
						.pipe(
							Effect.mapError((error) =>
								map_repository_error(decoded.payload.target_id, error),
							),
						);

					yield* publish(acceptance);

					return acceptance;
				});
			const Probe = (command: Command) =>
				Effect.gen(function* () {
					const decoded = yield* Schema.decodeUnknownEffect(CommandEnvelope)(
						command,
					).pipe(Effect.mapError(() => target_error("", "invalid_target")));

					if (!is_probe_command(decoded)) {
						return yield* Effect.fail(target_error("", "invalid_target"));
					}

					const ready = yield* AwaitProbeClaim(decoded);

					if (ready._tag === "Completed") {
						return ready.acceptance;
					}

					const { claim } = ready;

					return yield* Effect.gen(function* () {
						const health = yield* health_probe.Probe(claim.target).pipe(
							Effect.timeout(probe_timeout_ms),
							Effect.mapError(() =>
								target_error(decoded.payload.target_id, "health_probe"),
							),
						);
						const now_ms = yield* clock.Now;
						const acceptance = yield* repository
							.CompleteProbe(decoded, claim, health, now_ms)
							.pipe(
								Effect.mapError((error) =>
									map_repository_error(decoded.payload.target_id, error),
								),
							);

						yield* publish(acceptance);

						return acceptance;
					}).pipe(Effect.ensuring(repository.ReleaseProbe(claim).pipe(Effect.ignore)));
				});

			return {
				Get: (input) =>
					repository
						.Get(input)
						.pipe(
							Effect.mapError((error) =>
								map_repository_error(input.target_id, error),
							),
						),
				List: (input) =>
					repository
						.List(input)
						.pipe(Effect.mapError((error) => map_repository_error("", error))),
				Probe,
				Register,
				Remove,
				SlidingEvents: Stream.fromPubSub(events),
			};
		}),
	);
}

/** Provides wall-clock timestamps for preview target production composition. */
export const PreviewTargetClockLive = Layer.succeed(PreviewTargetClock, {
	Now: Clock.currentTimeMillis,
});
