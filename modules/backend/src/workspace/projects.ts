import {
	Context,
	Data,
	Deferred,
	Effect,
	Exit,
	Layer,
	Option,
	PubSub,
	Queue,
	Ref,
	Scope,
	TxReentrantLock,
} from "effect";

import { WorkspaceBoundedRegularFileStoreRegistry } from "../filesystem/workspace-bounded-regular-file-store-registry";
import { WorkspaceFilesystemRegistry } from "../filesystem/workspace-filesystem-registry";
import { WorkspaceGitRegistry } from "../git/workspace-git-registry";
import { ProjectCatalog } from "../projects/project-catalog";

/** Reports a catalog-to-workspace reconciliation that could not reach authority. */
export class ProjectWorkspaceBindingError extends Data.TaggedError("ProjectWorkspaceBindingError")<{
	readonly cause: unknown;
	readonly code: "reconcile_failed";
}> {}

/**
 * The one scoped result for authoritative project workspace binding.
 *
 * Durable catalog operations may proceed while the first flight runs. Any
 * operation that needs the filesystem or bounded-store authority must await
 * this gate before consulting those registries. A failed flight is stable
 * until an explicit Retry starts exactly one replacement flight.
 */
export class ProjectWorkspaceBindingGate extends Context.Service<
	ProjectWorkspaceBindingGate,
	{
		readonly Await: Effect.Effect<void, ProjectWorkspaceBindingError>;
		readonly Retry: Effect.Effect<void>;
		readonly Use: <A, E, R>(
			operation: Effect.Effect<A, E, R>,
		) => Effect.Effect<A, E | ProjectWorkspaceBindingError, R>;
	}
>()("Artisan/ProjectWorkspaceBindingGate") {}

/** Deterministic observation used only by lifecycle coverage. */
export interface ProjectWorkspaceBindingOptions {
	readonly OnBeforeReconcile?: Effect.Effect<void, ProjectWorkspaceBindingError>;
	/** Test-only observation after a new generation is published but before its predecessor settles. */
	readonly OnChangePublished?: Effect.Effect<void>;
	/** Test-only observation after a catalog event owns the new gate generation. */
	readonly OnChangeClaimed?: Effect.Effect<void>;
	/** Test-only observation between the retry claim and its scoped driver admission. */
	readonly OnRetryAdmission?: Effect.Effect<void>;
}

/** Binds every attached project to the filesystem, Git, and bounded-store registries. */
const MakeProjectWorkspaceBinding = (options: ProjectWorkspaceBindingOptions = {}) =>
	Effect.gen(function* () {
		const catalog = yield* ProjectCatalog;
		const filesystems = yield* WorkspaceFilesystemRegistry;
		const git = yield* WorkspaceGitRegistry;
		const stores = yield* WorkspaceBoundedRegularFileStoreRegistry;

		const BindSnapshot = catalog.Snapshot.pipe(
			Effect.andThen((snapshot) => {
				const registrations = snapshot.projects.map((project) => ({
					root: project.root_path,
					workspace_id: project.project_id,
				}));

				return (options.OnBeforeReconcile ?? Effect.void).pipe(
					Effect.andThen(filesystems.Reconcile(registrations)),
					/**
					 * The same roots, on the same terms. Git was the one workspace
					 * authority this binding never reconciled, so its registry only
					 * ever held what construction passed it — nothing, in production —
					 * and every workspace-scoped Git read failed "not found". A project
					 * that is not a repository is dropped by the reconcile itself, so
					 * it costs a warning rather than the whole snapshot.
					 */
					Effect.andThen(git.Reconcile(registrations)),
					Effect.andThen(stores.Reconcile),
				);
			}),
			Effect.mapError(
				(cause) => new ProjectWorkspaceBindingError({ cause, code: "reconcile_failed" }),
			),
		);

		return { BindSnapshot };
	});

/** Performs one authoritative binding snapshot for direct callers and tests. */
export const BindProjectWorkspaces = (options: ProjectWorkspaceBindingOptions = {}) =>
	Effect.gen(function* () {
		const binding = yield* MakeProjectWorkspaceBinding(options);
		yield* binding.BindSnapshot;
		return binding;
	});

interface BindingFlight {
	readonly result: Deferred.Deferred<BindingResolution>;
	readonly status: "failed" | "ready" | "running";
}

type BindingResolution =
	| { readonly _tag: "settled"; readonly exit: Exit.Exit<void, ProjectWorkspaceBindingError> }
	| { readonly _tag: "superseded" };

/**
 * Keeps project authority current without putting catalog or filesystem work on
 * Forge's construction critical path. The subscription is acquired before the
 * initial snapshot, preserving the detach-race authority ordering.
 */
export const make_project_workspace_binding_layer = (
	options: ProjectWorkspaceBindingOptions = {},
) =>
	Layer.effect(
		ProjectWorkspaceBindingGate,
		Effect.gen(function* () {
			const catalog = yield* ProjectCatalog;
			const changes = yield* catalog.Subscribe;
			const scope = yield* Scope.Scope;
			const binding = yield* MakeProjectWorkspaceBinding(options);
			const authority_lock = yield* TxReentrantLock.make();
			const initial = yield* Deferred.make<BindingResolution>();
			const flight = yield* Ref.make<BindingFlight>({ result: initial, status: "running" });
			const pending = yield* Queue.unbounded<Deferred.Deferred<BindingResolution>>();
			yield* Effect.addFinalizer(() => Queue.shutdown(pending));

			const AwaitReady: Effect.Effect<BindingFlight, ProjectWorkspaceBindingError> =
				Effect.suspend(() =>
					Ref.get(flight).pipe(
						Effect.flatMap((expected) =>
							Deferred.await(expected.result).pipe(
								Effect.map((resolution) => ({ expected, resolution })),
							),
						),
						Effect.flatMap(({ expected, resolution }) => {
							if (resolution._tag === "superseded") return AwaitReady;
							return Exit.isSuccess(resolution.exit)
								? Effect.succeed(expected)
								: Effect.failCause(resolution.exit.cause);
						}),
					),
				);
			const Await = AwaitReady.pipe(Effect.asVoid);
			const Use = <A, E, R>(
				operation: Effect.Effect<A, E, R>,
			): Effect.Effect<A, E | ProjectWorkspaceBindingError, R> =>
				AwaitReady.pipe(
					Effect.flatMap((expected) =>
						TxReentrantLock.withReadLock(
							authority_lock,
							Ref.get(flight).pipe(
								Effect.flatMap((current) =>
									current.result === expected.result && current.status === "ready"
										? operation.pipe(Effect.map(Option.some))
										: Effect.succeed(Option.none<A>()),
								),
							),
						),
					),
					Effect.flatMap((result) =>
						Option.isSome(result) ? Effect.succeed(result.value) : Use(operation),
					),
				);
			const Complete = (
				result: Deferred.Deferred<BindingResolution>,
				exit: Exit.Exit<void, ProjectWorkspaceBindingError>,
			) =>
				Ref.modify(flight, (current) => {
					if (current.result !== result) return [false, current] as const;
					return [
						true,
						{
							result,
							status: (Exit.isSuccess(exit)
								? "ready"
								: "failed") as BindingFlight["status"],
						},
					] as const;
				}).pipe(
					Effect.flatMap((owned) =>
						owned
							? Deferred.succeed(result, { _tag: "settled", exit }).pipe(
									Effect.asVoid,
								)
							: Effect.void,
					),
				);
			const ClaimChange = Effect.uninterruptibleMask(() =>
				Effect.gen(function* () {
					const result = yield* Deferred.make<BindingResolution>();
					const claimed = yield* Ref.modify(flight, (current) => {
						if (current.status === "failed") return [Option.none(), current] as const;
						return [
							Option.some(current),
							{ result, status: "running" } satisfies BindingFlight,
						] as const;
					});
					if (Option.isNone(claimed))
						return Option.none<Deferred.Deferred<BindingResolution>>();
					yield* options.OnChangePublished ?? Effect.void;
					yield* Deferred.succeed(claimed.value.result, { _tag: "superseded" });
					yield* options.OnChangeClaimed ?? Effect.void;
					return Option.some(result);
				}),
			);
			const RunGeneration = (result: Deferred.Deferred<BindingResolution>) =>
				TxReentrantLock.withWriteLock(authority_lock, binding.BindSnapshot).pipe(
					Effect.asVoid,
					Effect.exit,
					Effect.tap((exit) => Complete(result, exit)),
				);
			const Observe = PubSub.take(changes).pipe(
				Effect.flatMap(() =>
					Effect.uninterruptibleMask(() =>
						ClaimChange.pipe(
							Effect.flatMap((claimed) =>
								Option.isSome(claimed)
									? Queue.offer(pending, claimed.value).pipe(Effect.asVoid)
									: Effect.void,
							),
						),
					),
				),
				Effect.forever,
			);
			const Work = (
				result: Deferred.Deferred<BindingResolution>,
			): Effect.Effect<void, ProjectWorkspaceBindingError> =>
				RunGeneration(result).pipe(
					Effect.flatMap((exit) =>
						Ref.get(flight).pipe(
							Effect.flatMap((current) =>
								Exit.isFailure(exit) && current.result === result
									? Effect.failCause(exit.cause)
									: Queue.take(pending).pipe(Effect.flatMap(Work)),
							),
						),
					),
				);
			const Drive = (result: Deferred.Deferred<BindingResolution>) =>
				Work(result).pipe(
					/** Scope shutdown must settle every outstanding waiter before this fiber exits. */
					Effect.onExit((exit) =>
						Exit.isFailure(exit)
							? Complete(result, Exit.failCause(exit.cause)).pipe(
									Effect.asVoid,
									Effect.uninterruptible,
								)
							: Effect.void,
					),
				);
			const Start = (result: Deferred.Deferred<BindingResolution>) =>
				Effect.forkIn(Drive(result), scope);
			yield* Effect.addFinalizer(() =>
				Ref.get(flight).pipe(
					Effect.flatMap((current) =>
						current.status === "running"
							? Effect.exit(Effect.interrupt).pipe(
									Effect.flatMap((interrupted) =>
										Complete(current.result, interrupted),
									),
								)
							: Effect.void,
					),
				),
			);

			yield* Effect.forkIn(Observe, scope);
			yield* Start(initial);

			const Retry = Effect.uninterruptibleMask(() =>
				Effect.gen(function* () {
					const candidate = yield* Deferred.make<BindingResolution>();
					const start = yield* Ref.modify(flight, (current) => {
						if (current.status !== "failed") return [false, current] as const;
						return [true, { result: candidate, status: "running" }] as const;
					});
					if (!start) return;
					yield* options.OnRetryAdmission ?? Effect.void;
					yield* Start(candidate);
				}),
			);

			return { Await, Retry, Use };
		}),
	);

/** Production binding coordinator. It returns immediately; workspace users await its gate. */
export const ProjectWorkspaceBindingLive = make_project_workspace_binding_layer();
