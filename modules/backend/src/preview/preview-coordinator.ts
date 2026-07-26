import { createHash } from "node:crypto";

import { Clock, Context, Effect, Layer, Option, Ref } from "effect";

import type {
	PreviewBrowserLaunch,
	PreviewInspectionResult,
	PreviewInspectionSession,
	PreviewTarget,
	RichLinkResolution,
} from "@artisan/protocol";

import { RichLinkAssetStore } from "./rich-link-asset-store";
import { RichLinkMetadata } from "./rich-link-metadata";
import { PreviewExternalBrowser, PreviewInspection, PreviewRuntimeError } from "./preview-runtime";
import {
	PreviewRepositoryError,
	PreviewRepository,
	type PreviewInspectionProjection,
	type PreviewDispatchLease,
	type PreviewTargetProjection,
} from "./preview-repository";
import { PreviewService } from "./preview-service";
import {
	PreviewHealthProbe,
	PreviewHealthProbeError,
	PreviewTarget as PreviewTargetRuntime,
} from "./preview-target";
import { MakeThreadDispatchFence } from "../threads/internal/thread-dispatch-fence";

export type PreviewCoordinatorError =
	| PreviewHealthProbeError
	| PreviewRepositoryError
	| PreviewRuntimeError;

/** Coordinates durable preview facts with the explicitly scoped runtime effects. */
export class PreviewCoordinator extends Context.Service<
	PreviewCoordinator,
	{
		readonly Asset: (asset_id: string) => Effect.Effect<
			Option.Option<{
				readonly asset_id: string;
				readonly body: Uint8Array;
				readonly bytes: number;
				readonly content_type: string;
			}>
		>;
		readonly AssetMetadata: (
			asset_id: string,
		) => Effect.Effect<
			| { readonly asset_id: string; readonly bytes: number; readonly content_type: string }
			| undefined
		>;
		readonly CloseInspection: (input: {
			readonly message_id: string;
			readonly session_id: string;
		}) => Effect.Effect<PreviewInspectionSession, PreviewCoordinatorError>;
		readonly Get: (target_id: string) => Effect.Effect<PreviewTarget, PreviewCoordinatorError>;
		readonly Inspect: (input: {
			readonly message_id: string;
			readonly operation: "health" | "metadata";
			readonly session_id: string;
		}) => Effect.Effect<PreviewInspectionResult, PreviewCoordinatorError>;
		readonly Launch: (input: {
			readonly message_id: string;
			readonly target_id: string;
		}) => Effect.Effect<PreviewBrowserLaunch, PreviewCoordinatorError>;
		readonly List: (
			workspace_id?: string,
		) => Effect.Effect<ReadonlyArray<PreviewTarget>, PreviewCoordinatorError>;
		readonly OpenInspection: (input: {
			readonly connector_id: string;
			readonly message_id: string;
			readonly target_id: string;
		}) => Effect.Effect<PreviewInspectionSession, PreviewCoordinatorError>;
		readonly Probe: (input: {
			readonly message_id: string;
			readonly target_id: string;
		}) => Effect.Effect<PreviewTarget, PreviewCoordinatorError>;
		/** Fences runtime-only preview resources before the owning thread rows are erased. */
		readonly QuiesceThread: (thread_id: string) => Effect.Effect<void, PreviewCoordinatorError>;
		readonly Register: (input: {
			readonly id: string;
			readonly message_id: string;
			readonly port: number;
			readonly project_id: string;
			readonly routes: ReadonlyArray<string>;
			readonly source?:
				| { readonly kind: "process"; readonly process_id: string }
				| { readonly kind: "terminal"; readonly terminal_id: string };
			readonly thread_id: string;
			readonly url: string;
			readonly workspace_id: string;
		}) => Effect.Effect<PreviewTarget, PreviewCoordinatorError>;
		readonly Remove: (input: {
			readonly message_id: string;
			readonly target_id: string;
			readonly thread_id: string;
		}) => Effect.Effect<PreviewTarget, PreviewCoordinatorError>;
		readonly ResolveRichLink: (url: string) => Effect.Effect<RichLinkResolution, unknown>;
		readonly SetState: (input: {
			readonly message_id: string;
			readonly state: "healthy" | "registered" | "stopped" | "unhealthy";
			readonly target_id: string;
			readonly thread_id: string;
		}) => Effect.Effect<PreviewTarget, PreviewCoordinatorError>;
	}
>()("Artisan/PreviewCoordinator") {}

function internal_message_id(message_id: string, phase: string) {
	return `preview:${phase}:${createHash("sha256").update(message_id).digest("hex")}`;
}

function parse_optional_json<T>(value: string | null): T | undefined {
	if (value === null) return undefined;
	try {
		return JSON.parse(value) as T;
	} catch {
		return undefined;
	}
}

function to_target(value: PreviewTargetProjection): PreviewTarget {
	return {
		created_at: value.created_at,
		...(value.health_json === null
			? {}
			: { health: parse_optional_json<PreviewTarget["health"]>(value.health_json) }),
		id: value.target_id,
		journal_sequence: value.journal_sequence,
		...(value.last_error === null ? {} : { last_error: value.last_error }),
		launch_state: value.launch_state,
		port: value.port,
		project_id: value.project_id,
		routes: parse_optional_json<ReadonlyArray<string>>(value.routes_json) ?? [],
		...(value.source === undefined ? {} : { source: value.source }),
		state: value.state === "removed" ? "stopped" : value.state,
		thread_id: value.thread_id,
		updated_at: value.updated_at,
		url: value.url,
		workspace_id: value.workspace_id,
	};
}

function to_session(value: PreviewInspectionProjection): PreviewInspectionSession {
	return {
		...(value.closed_at === null ? {} : { closed_at: value.closed_at }),
		connector_id: value.connector_id,
		...(value.last_error === null ? {} : { last_error: value.last_error }),
		opened_at: value.opened_at,
		reconnect_state: value.reconnect_state,
		session_id: value.session_id,
		state: value.state,
		target_id: value.target_id,
		updated_at: value.updated_at,
	};
}

/** Production coordinator. Intent is stored before an opener or inspection connector can run. */
export const PreviewCoordinatorLive = Layer.effect(
	PreviewCoordinator,
	Effect.gen(function* () {
		const assets = yield* RichLinkAssetStore;
		const browser = yield* PreviewExternalBrowser;
		const health_probe = yield* PreviewHealthProbe;
		const inspection = yield* PreviewInspection;
		const metadata = yield* RichLinkMetadata;
		const runtime_targets = yield* PreviewTargetRuntime;
		const repository = yield* PreviewRepository;
		const service = yield* PreviewService;
		const dispatch_fence = yield* MakeThreadDispatchFence;
		const inspection_handles = yield* Ref.make(new Map<string, string>());
		const Dispatch = <A, E, R>(
			input: {
				readonly kind: PreviewDispatchLease["kind"];
				readonly session_id?: string;
				readonly target_id?: string;
				readonly thread_id: string;
			},
			operation: (lease: PreviewDispatchLease) => Effect.Effect<A, E, R>,
		) =>
			dispatch_fence
				.Run(
					input.thread_id,
					service
						.AcquireDispatchLease(input)
						.pipe(
							Effect.flatMap((lease) =>
								Effect.raceFirst(
									operation(lease),
									Effect.sleep("20 seconds").pipe(
										Effect.andThen(service.RenewDispatchLease(lease)),
										Effect.forever,
									),
								).pipe(Effect.ensuring(service.ReleaseDispatchLease(lease))),
							),
						),
				)
				.pipe(
					Effect.flatMap((result) =>
						Option.isSome(result)
							? Effect.succeed(result.value)
							: Effect.fail(
									new PreviewRepositoryError({
										code: "not_found",
										message: "Thread is unavailable for preview dispatch",
									}),
								),
					),
				);
		/** Runtime handles are intentionally reconstructed only as local target records. Durable
		 * browser launches and connector sessions are never replayed after a backend restart. */
		const persisted_targets = yield* service.List();
		yield* Effect.forEach(
			persisted_targets.filter((target) => target.state !== "removed"),
			(target) =>
				runtime_targets
					.Register({
						id: target.target_id,
						project_id: target.project_id,
						url: target.url,
						workspace_id: target.workspace_id,
					})
					.pipe(Effect.ignore),
			{ discard: true },
		);
		const stale_dispatches = yield* service.RecoverDispatchLeases();
		yield* Effect.forEach(
			stale_dispatches,
			(lease) =>
				lease.kind === "launch" && lease.target_id !== null
					? service
							.UpdateTarget({
								action: "launch",
								last_error: "dispatch_lease_expired",
								launch_state: "error",
								message_id: `preview:recovery:launch:${lease.lease_id}`,
								target_id: lease.target_id,
								thread_id: lease.thread_id,
							})
							.pipe(Effect.ignore)
					: Effect.void,
			{ discard: true },
		);
		yield* service.RecoverInspections();

		const Get = (target_id: string) => service.Get(target_id).pipe(Effect.map(to_target));
		const List = (workspace_id?: string) =>
			service
				.List(workspace_id)
				.pipe(
					Effect.map((targets) =>
						targets.filter((target) => target.state !== "removed").map(to_target),
					),
				);
		const Register = (input: {
			readonly id: string;
			readonly message_id: string;
			readonly port: number;
			readonly project_id: string;
			readonly routes: ReadonlyArray<string>;
			readonly source?:
				| { readonly kind: "process"; readonly process_id: string }
				| { readonly kind: "terminal"; readonly terminal_id: string };
			readonly thread_id: string;
			readonly url: string;
			readonly workspace_id: string;
		}) =>
			dispatch_fence
				.Run(
					input.thread_id,
					service
						.Register({
							message_id: input.message_id,
							port: input.port,
							project_id: input.project_id,
							routes: input.routes,
							...(input.source === undefined ? {} : { source: input.source }),
							target_id: input.id,
							thread_id: input.thread_id,
							url: input.url,
							workspace_id: input.workspace_id,
						})
						.pipe(
							Effect.tap((target) =>
								runtime_targets
									.Register({
										id: target.target_id,
										project_id: target.project_id,
										url: target.url,
										workspace_id: target.workspace_id,
									})
									.pipe(Effect.ignore),
							),
							Effect.map(to_target),
						),
				)
				.pipe(
					Effect.flatMap((result) =>
						Option.isSome(result)
							? Effect.succeed(result.value)
							: Effect.fail(
									new PreviewRepositoryError({
										code: "not_found",
										message: "Thread is unavailable for preview registration",
									}),
								),
					),
				);
		const Probe = (input: { readonly message_id: string; readonly target_id: string }) =>
			Effect.gen(function* () {
				const target = yield* service.Get(input.target_id);
				return yield* Dispatch(
					{ kind: "probe", target_id: input.target_id, thread_id: target.thread_id },
					(lease) =>
						Effect.gen(function* () {
							const runtime_target = {
								created_at_ms: Date.parse(target.created_at),
								health: Option.none(),
								id: target.target_id,
								project_id: target.project_id,
								source: Option.none(),
								state: target.state === "removed" ? "stopped" : target.state,
								updated_at_ms: Date.parse(target.updated_at),
								url: target.url,
								workspace_id: target.workspace_id,
							} as const;
							const observed = yield* Effect.scoped(
								health_probe.Probe(runtime_target),
							).pipe(
								Effect.catch((error) =>
									service
										.UpdateTarget(
											{
												action: "probe",
												last_error: "health_probe_unavailable",
												message_id: internal_message_id(
													input.message_id,
													"probe-error",
												),
												state: "unhealthy",
												target_id: input.target_id,
												thread_id: target.thread_id,
											},
											lease.lease_id,
										)
										.pipe(Effect.andThen(Effect.fail(error))),
								),
							);
							const updated = yield* service.UpdateTarget(
								{
									action: "probe",
									health_json: JSON.stringify({
										checked_at: new Date(
											yield* Clock.currentTimeMillis,
										).toISOString(),
										...observed,
										message: Option.getOrUndefined(observed.message),
										status_code: Option.getOrUndefined(observed.status_code),
									}),
									message_id: input.message_id,
									state: observed.status,
									target_id: input.target_id,
									thread_id: target.thread_id,
								},
								lease.lease_id,
							);
							return to_target(updated);
						}),
				);
			});
		const SetState = (input: {
			readonly message_id: string;
			readonly state: "healthy" | "registered" | "stopped" | "unhealthy";
			readonly target_id: string;
			readonly thread_id: string;
		}) =>
			service
				.UpdateTarget({
					action: "state",
					message_id: input.message_id,
					state: input.state,
					target_id: input.target_id,
					thread_id: input.thread_id,
				})
				.pipe(Effect.map(to_target));
		const Remove = (input: {
			readonly message_id: string;
			readonly target_id: string;
			readonly thread_id: string;
		}) =>
			service
				.UpdateTarget({
					action: "remove",
					message_id: input.message_id,
					target_id: input.target_id,
					thread_id: input.thread_id,
				})
				.pipe(
					Effect.tap(() => runtime_targets.Remove(input.target_id).pipe(Effect.ignore)),
					Effect.map(to_target),
				);
		const Launch = (input: { readonly message_id: string; readonly target_id: string }) =>
			Effect.gen(function* () {
				const before = yield* service.Get(input.target_id);
				return yield* Dispatch(
					{ kind: "launch", target_id: input.target_id, thread_id: before.thread_id },
					(lease) =>
						Effect.gen(function* () {
							const result_update = {
								action: "launch" as const,
								launch_state: "launched" as const,
								message_id: internal_message_id(input.message_id, "launch-result"),
								target_id: input.target_id,
								thread_id: before.thread_id,
							};
							const replay = yield* service.ReplayTargetUpdate(result_update);
							if (Option.isSome(replay))
								return {
									launched_at: replay.value.updated_at,
									target_id: input.target_id,
								};
							const error_update = {
								action: "launch" as const,
								last_error: "external_browser_unavailable",
								launch_state: "error" as const,
								message_id: internal_message_id(input.message_id, "launch-error"),
								target_id: input.target_id,
								thread_id: before.thread_id,
							};
							const failure_replay = yield* service.ReplayTargetUpdate(error_update);
							if (Option.isSome(failure_replay))
								return yield* Effect.fail(
									new PreviewRuntimeError({
										cause: new Error(
											"external browser launch previously failed",
										),
										code: "browser_unavailable",
										target_id: input.target_id,
									}),
								);
							const intent_update = {
								action: "launch" as const,
								launch_state: "launching" as const,
								message_id: internal_message_id(input.message_id, "launch-intent"),
								target_id: input.target_id,
								thread_id: before.thread_id,
							};
							const intent_replay = yield* service.ReplayTargetUpdate(intent_update);
							if (Option.isSome(intent_replay))
								return yield* Effect.fail(
									new PreviewRuntimeError({
										cause: new Error(
											"external browser launch outcome is ambiguous",
										),
										code: "browser_unavailable",
										target_id: input.target_id,
									}),
								);
							if (
								before.launch_state === "launching" ||
								before.launch_state === "launched"
							)
								return yield* Effect.fail(
									new PreviewRepositoryError({
										code: "invalid",
										message:
											"External browser launch is already in progress or was already dispatched",
									}),
								);
							yield* service.UpdateTarget(intent_update, lease.lease_id);
							const completed = yield* Effect.scoped(
								browser.Launch({
									actor_id: "frontend",
									target_id: input.target_id,
								}),
							).pipe(
								Effect.flatMap(() =>
									service.UpdateTarget(result_update, lease.lease_id),
								),
								Effect.catch(() =>
									service.UpdateTarget(error_update, lease.lease_id).pipe(
										Effect.andThen(
											Effect.fail(
												new PreviewRuntimeError({
													cause: new Error(
														"external browser launch failed",
													),
													code: "browser_unavailable",
													target_id: input.target_id,
												}),
											),
										),
									),
								),
							);
							return {
								launched_at: completed.updated_at,
								target_id: input.target_id,
							};
						}),
				);
			});
		const OpenInspection = (input: {
			readonly connector_id: string;
			readonly message_id: string;
			readonly target_id: string;
		}) =>
			Effect.gen(function* () {
				const target = yield* service.Get(input.target_id);
				const session_id = internal_message_id(input.message_id, "inspection");
				return yield* Dispatch(
					{
						kind: "inspection_open",
						session_id,
						target_id: input.target_id,
						thread_id: target.thread_id,
					},
					(lease) =>
						Effect.gen(function* () {
							const existing = (yield* repository.ListOpenInspections()).find(
								(session) => session.session_id === session_id,
							);
							if (existing) return to_session(existing);
							const persisted = yield* service.UpdateInspection(
								{
									action: "inspection_open",
									connector_id: input.connector_id,
									message_id: internal_message_id(
										input.message_id,
										"inspection-intent",
									),
									session_id,
									target_id: input.target_id,
									thread_id: target.thread_id,
								},
								lease.lease_id,
							);
							const runtime_session = yield* inspection
								.Open({
									actor_id: "frontend",
									connector_id: input.connector_id,
									target_id: input.target_id,
								})
								.pipe(
									Effect.catch((error) =>
										service
											.UpdateInspection(
												{
													action: "inspection_reconnect",
													last_error: "connector_unavailable",
													message_id: internal_message_id(
														input.message_id,
														"inspection-error",
													),
													reconnect_state: "unavailable",
													session_id,
													thread_id: target.thread_id,
												},
												lease.lease_id,
											)
											.pipe(Effect.andThen(Effect.fail(error))),
									),
								);
							yield* Ref.update(inspection_handles, (handles) =>
								new Map(handles).set(session_id, runtime_session.session_id),
							);
							return to_session(persisted);
						}),
				);
			});
		const Inspect = (input: {
			readonly message_id: string;
			readonly operation: "health" | "metadata";
			readonly session_id: string;
		}) =>
			Effect.gen(function* () {
				const sessions = yield* repository.ListOpenInspections();
				const session = sessions.find(
					(candidate) => candidate.session_id === input.session_id,
				);
				if (!session)
					return yield* Effect.fail(
						new PreviewRepositoryError({
							code: "not_found",
							message: "Preview inspection session is unavailable",
						}),
					);
				const runtime_session_id = (yield* Ref.get(inspection_handles)).get(
					input.session_id,
				);
				if (runtime_session_id === undefined)
					return yield* Effect.fail(
						new PreviewRuntimeError({
							cause: new Error("preview inspection connector is unavailable"),
							code: "not_found",
							target_id: session.target_id,
						}),
					);
				if (input.operation === "metadata")
					return {
						operation: "metadata" as const,
						session_id: session.session_id,
						target: yield* Get(session.target_id),
					};
				return yield* Dispatch(
					{
						kind: "inspection_health",
						session_id: session.session_id,
						target_id: session.target_id,
						thread_id: session.thread_id,
					},
					(lease) =>
						Effect.gen(function* () {
							const result = yield* Effect.scoped(
								inspection.Inspect(runtime_session_id),
							);
							yield* service.UpdateTarget(
								{
									action: "probe",
									health_json: JSON.stringify({
										checked_at: new Date(
											yield* Clock.currentTimeMillis,
										).toISOString(),
										latency_ms: result.latency_ms,
										message: Option.getOrUndefined(result.message),
										status: result.status,
										status_code: Option.getOrUndefined(result.status_code),
									}),
									message_id: internal_message_id(
										input.message_id,
										"inspection-health",
									),
									state: result.status,
									target_id: session.target_id,
									thread_id: session.thread_id,
								},
								lease.lease_id,
							);
							return {
								health: {
									checked_at: new Date(
										yield* Clock.currentTimeMillis,
									).toISOString(),
									latency_ms: result.latency_ms,
									...(Option.isSome(result.message)
										? { message: result.message.value }
										: {}),
									status: result.status,
									...(Option.isSome(result.status_code)
										? { status_code: result.status_code.value }
										: {}),
								},
								operation: "health" as const,
								session_id: session.session_id,
							};
						}),
				);
			});
		const CloseInspection = (input: {
			readonly message_id: string;
			readonly session_id: string;
		}) =>
			Effect.gen(function* () {
				const sessions = yield* repository.ListOpenInspections();
				const session = sessions.find(
					(candidate) => candidate.session_id === input.session_id,
				);
				if (!session)
					return yield* Effect.fail(
						new PreviewRepositoryError({
							code: "not_found",
							message: "Preview inspection session is unavailable",
						}),
					);
				const runtime_session_id = (yield* Ref.get(inspection_handles)).get(
					input.session_id,
				);
				if (runtime_session_id !== undefined)
					yield* inspection.Close(runtime_session_id).pipe(
						Effect.catch((error) =>
							Ref.update(inspection_handles, (handles) => {
								const next = new Map(handles);
								next.delete(input.session_id);
								return next;
							}).pipe(
								Effect.andThen(
									service.UpdateInspection({
										action: "inspection_reconnect",
										last_error: "connector_close_failed",
										message_id: internal_message_id(
											input.message_id,
											"inspection-close-error",
										),
										reconnect_state: "error",
										session_id: input.session_id,
										thread_id: session.thread_id,
									}),
								),
								Effect.andThen(Effect.fail(error)),
							),
						),
					);
				yield* Ref.update(inspection_handles, (handles) => {
					const next = new Map(handles);
					next.delete(input.session_id);
					return next;
				});
				const closed = yield* service.UpdateInspection({
					action: "inspection_close",
					message_id: input.message_id,
					session_id: input.session_id,
					thread_id: session.thread_id,
				});
				return to_session(closed);
			});
		const QuiesceThread = (thread_id: string) =>
			dispatch_fence.Quiesce(
				thread_id,
				Effect.gen(function* () {
					const targets = (yield* service.List()).filter(
						(target) => target.thread_id === thread_id,
					);
					/** Remove runtime records first: any concurrent opener or connector rechecks this
					 * registry before its external side effect and therefore fails closed. */
					yield* Effect.forEach(
						targets,
						(target) => runtime_targets.Remove(target.target_id).pipe(Effect.ignore),
						{ discard: true },
					);
					const sessions = (yield* repository.ListOpenInspections()).filter(
						(session) => session.thread_id === thread_id,
					);
					const handles = yield* Ref.get(inspection_handles);
					yield* Effect.forEach(
						sessions,
						(session) => {
							const runtime_session_id = handles.get(session.session_id);
							return runtime_session_id === undefined
								? Effect.void
								: inspection.Close(runtime_session_id);
						},
						{ discard: true },
					);
					yield* Ref.update(inspection_handles, (current) => {
						const next = new Map(current);
						for (const session of sessions) next.delete(session.session_id);
						return next;
					});
				}).pipe(Effect.catchCause(Effect.die)),
			);
		return {
			Asset: assets.Get,
			AssetMetadata: (asset_id) =>
				assets.Get(asset_id).pipe(
					Effect.map((asset) =>
						Option.isSome(asset)
							? {
									asset_id: asset.value.asset_id,
									bytes: asset.value.bytes,
									content_type: asset.value.content_type,
								}
							: undefined,
					),
				),
			CloseInspection,
			Get,
			Inspect,
			Launch,
			List,
			OpenInspection,
			Probe,
			QuiesceThread,
			Register,
			Remove,
			ResolveRichLink: (url) =>
				metadata.Resolve(url).pipe(
					Effect.map((result) => ({
						...result,
						cache: {
							expires_at: new Date(result.cache.expires_at_ms).toISOString(),
							status: result.cache.status,
						},
						fetched_at: new Date(result.fetched_at_ms).toISOString(),
						favicon: Option.getOrUndefined(result.favicon),
						title: Option.getOrUndefined(result.title),
					})),
				),
			SetState,
		};
	}),
);
