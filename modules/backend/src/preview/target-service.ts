import { Clock, Effect, Layer, Option, PubSub, Ref, Stream } from "effect";

import { is_local_preview_hostname } from "./network-policy";
import {
	PreviewHealthProbe,
	PreviewTarget,
	PreviewTargetClock,
	PreviewTargetError,
	type PreviewTargetEvent,
	type PreviewTargetRecord,
	type PreviewTargetRegistration,
	type PreviewTargetState,
} from "./target";

function target_error(target_id: string, code: PreviewTargetError["code"], cause: unknown) {
	return new PreviewTargetError({ cause, code, target_id });
}

function has_nonempty_id(value: unknown): value is string {
	return typeof value === "string" && value.trim().length > 0;
}

function parse_local_preview_url(input: PreviewTargetRegistration) {
	return Effect.gen(function* () {
		if (
			!has_nonempty_id(input.id) ||
			!has_nonempty_id(input.project_id) ||
			!has_nonempty_id(input.workspace_id)
		) {
			return yield* Effect.fail(
				target_error(
					input.id,
					"invalid_target",
					new Error("target, project, and workspace IDs must be nonempty"),
				),
			);
		}

		const url = yield* Effect.try({
			try: () => new URL(input.url),
			catch: (cause) => target_error(input.id, "invalid_target", cause),
		});

		if (url.protocol !== "http:" && url.protocol !== "https:") {
			return yield* Effect.fail(
				target_error(input.id, "invalid_target", new Error("preview URL must use HTTP(S)")),
			);
		}

		if (url.username || url.password || !is_local_preview_hostname(url.hostname)) {
			return yield* Effect.fail(
				target_error(
					input.id,
					"invalid_target",
					new Error("preview URL must be an explicit localhost or loopback target"),
				),
			);
		}

		return url;
	});
}

/** Builds an in-memory preview target read model with scoped health probes. */
export function make_preview_target_layer() {
	return Layer.effect(
		PreviewTarget,
		Effect.gen(function* () {
			const clock = yield* PreviewTargetClock;
			const health_probe = yield* PreviewHealthProbe;
			const records = yield* Ref.make(new Map<string, PreviewTargetRecord>());
			const events = yield* Effect.acquireRelease(
				PubSub.unbounded<PreviewTargetEvent>(),
				PubSub.shutdown,
			);

			const publish = (event: PreviewTargetEvent) =>
				PubSub.publish(events, event).pipe(Effect.asVoid);
			const get_required = (id: string) =>
				Ref.get(records).pipe(
					Effect.flatMap((current) => {
						const target = current.get(id);

						return target
							? Effect.succeed(target)
							: Effect.fail(
									target_error(
										id,
										"not_found",
										new Error("preview target not found"),
									),
								);
					}),
				);
			const update = (
				id: string,
				kind: PreviewTargetEvent["kind"],
				update: (target: PreviewTargetRecord, now: number) => PreviewTargetRecord,
			) =>
				Effect.gen(function* () {
					const now = yield* clock.Now;
					const target = yield* Ref.modify(records, (current) => {
						const existing = current.get(id);

						if (!existing) {
							return [Option.none<PreviewTargetRecord>(), current] as const;
						}

						const next = update(existing, now);

						return [Option.some(next), new Map(current).set(id, next)] as const;
					});

					if (Option.isNone(target)) {
						return yield* Effect.fail(
							target_error(id, "not_found", new Error("preview target not found")),
						);
					}

					yield* publish({ kind, target: target.value });

					return target.value;
				});

			const register = (input: PreviewTargetRegistration) =>
				Effect.gen(function* () {
					const url = yield* parse_local_preview_url(input);
					const now = yield* clock.Now;
					const target: PreviewTargetRecord = {
						created_at_ms: now,
						health: Option.none(),
						id: input.id,
						project_id: input.project_id,
						source: Option.fromUndefinedOr(input.source),
						state: "registered",
						updated_at_ms: now,
						url: url.href,
						workspace_id: input.workspace_id,
					};
					const inserted = yield* Ref.modify(records, (current) =>
						current.has(target.id)
							? ([false, current] as const)
							: ([true, new Map(current).set(target.id, target)] as const),
					);

					if (!inserted) {
						return yield* Effect.fail(
							target_error(
								target.id,
								"duplicate",
								new Error("preview target already exists"),
							),
						);
					}

					yield* publish({ kind: "registered", target });

					return target;
				});

			const remove = (id: string) =>
				Effect.gen(function* () {
					const removed = yield* Ref.modify(records, (current) => {
						const target = current.get(id);

						if (!target) {
							return [Option.none<PreviewTargetRecord>(), current] as const;
						}

						const next = new Map(current);

						next.delete(id);

						return [Option.some(target), next] as const;
					});

					if (Option.isNone(removed)) {
						return yield* Effect.fail(
							target_error(id, "not_found", new Error("preview target not found")),
						);
					}

					yield* publish({ kind: "removed", target: removed.value });
				});

			const probe = (id: string) =>
				Effect.gen(function* () {
					const target = yield* get_required(id);
					const observation = yield* health_probe
						.Probe(target)
						.pipe(Effect.mapError((cause) => target_error(id, "health_probe", cause)));

					return yield* update(id, "health", (current, now) => ({
						...current,
						health: Option.some({ ...observation, checked_at_ms: now }),
						state: observation.status,
						updated_at_ms: now,
					}));
				});

			return {
				SlidingEvents: Stream.fromPubSub(events),
				Get: (id) =>
					Ref.get(records).pipe(
						Effect.map((current) => Option.fromUndefinedOr(current.get(id))),
					),
				List: (workspace_id) =>
					Ref.get(records).pipe(
						Effect.map((current) =>
							[...current.values()]
								.filter(
									(target) =>
										workspace_id === undefined ||
										target.workspace_id === workspace_id,
								)
								.toSorted((left, right) => left.id.localeCompare(right.id)),
						),
					),
				Probe: probe,
				Register: register,
				Remove: remove,
				SetState: (id: string, state: PreviewTargetState) =>
					update(id, "state", (target, now) => ({
						...target,
						state,
						updated_at_ms: now,
					})),
			};
		}),
	);
}

/** Provides wall-clock timestamps for preview target production composition. */
export const PreviewTargetClockLive = Layer.succeed(PreviewTargetClock, {
	Now: Clock.currentTimeMillis,
});
