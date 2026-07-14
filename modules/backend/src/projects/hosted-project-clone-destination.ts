import { Context, Data, Effect, FileSystem, Layer, Path, Schema } from "effect";

import {
	GitProviderCloneDestinationProof,
	GitProviderNativePath,
	type GitProviderCloneDestinationProof as GitProviderCloneDestinationProofValue,
} from "../git-provider/git-provider";
import {
	ReadFileIdentity,
	same_file_identity,
	type FileIdentity,
} from "../filesystem/file-identity";

/** Pins the approved projects root and visible empty destination before approval. */
export const HostedProjectCloneDestinationPlan = GitProviderCloneDestinationProof;

export type HostedProjectCloneDestinationPlan = typeof HostedProjectCloneDestinationPlan.Type;

/** Reports a destination failure without exposing platform error details. */
export class HostedProjectCloneDestinationError extends Data.TaggedError(
	"HostedProjectCloneDestinationError",
)<{
	readonly reason:
		| "destination_not_empty"
		| "destination_unavailable"
		| "invalid_destination"
		| "projects_root_unavailable";
}> {}

/** Configures the one user-approved root that may receive hosted project clones. */
export interface HostedProjectCloneDestinationOptions {
	readonly projects_root?: string;
}

/** Owns approval-time identity binding for one visible clone destination. */
export class HostedProjectCloneDestination extends Context.Service<
	HostedProjectCloneDestination,
	{
		readonly Plan: (
			destination_path: unknown,
		) => Effect.Effect<HostedProjectCloneDestinationPlan, HostedProjectCloneDestinationError>;
		readonly WithPinned: <A, E, R>(
			plan: HostedProjectCloneDestinationPlan,
			use: (destination: GitProviderCloneDestinationProofValue) => Effect.Effect<A, E, R>,
		) => Effect.Effect<A, E | HostedProjectCloneDestinationError, R>;
	}
>()("Artisan/HostedProjectCloneDestination") {}

interface PinnedPath {
	readonly canonical_path: string;
	readonly identity: FileIdentity;
}

function canonical_path_key(path_service: Path.Path, value: string) {
	const normalized = path_service.normalize(value);

	return process.platform === "win32" ? normalized.toLowerCase() : normalized;
}

function same_canonical_path(path_service: Path.Path, left: string, right: string) {
	return canonical_path_key(path_service, left) === canonical_path_key(path_service, right);
}

function decode_identity(device: string, inode: string): FileIdentity {
	return { device: BigInt(device), inode: BigInt(inode) };
}

function destination_error(reason: HostedProjectCloneDestinationError["reason"]) {
	return new HostedProjectCloneDestinationError({ reason });
}

/** Builds the Effect service that confines clone destinations to one projects root. */
export function make_hosted_project_clone_destination_layer(
	options: HostedProjectCloneDestinationOptions,
) {
	return Layer.effect(
		HostedProjectCloneDestination,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			const path_service = yield* Path.Path;

			const ReadDirectory = (
				path: string,
				reason: HostedProjectCloneDestinationError["reason"],
			) =>
				Effect.gen(function* () {
					const canonical_path = yield* file_system
						.realPath(path)
						.pipe(Effect.mapError(() => destination_error(reason)));
					const handle = yield* file_system
						.open(canonical_path, { flag: "r" })
						.pipe(Effect.mapError(() => destination_error(reason)));
					const info = yield* handle.stat.pipe(
						Effect.mapError(() => destination_error(reason)),
					);
					const identity = yield* ReadFileIdentity(handle.fd).pipe(
						Effect.mapError(() => destination_error(reason)),
					);

					if (info.type !== "Directory") {
						return yield* Effect.fail(destination_error(reason));
					}

					return { canonical_path, identity } satisfies PinnedPath;
				});

			const ReadProjectsRoot = Effect.gen(function* () {
				const configured_root = options.projects_root;

				if (configured_root === undefined || !path_service.isAbsolute(configured_root)) {
					return yield* Effect.fail(destination_error("projects_root_unavailable"));
				}

				const requested_root = path_service.resolve(configured_root);
				const root = yield* ReadDirectory(configured_root, "projects_root_unavailable");

				if (!same_canonical_path(path_service, requested_root, root.canonical_path)) {
					return yield* Effect.fail(destination_error("projects_root_unavailable"));
				}

				return root;
			});

			const ReadDestination = (
				unknown_destination: unknown,
				projects_root: string,
				require_empty: boolean,
			) =>
				Effect.gen(function* () {
					const destination_path = yield* Schema.decodeUnknownEffect(
						GitProviderNativePath,
					)(unknown_destination).pipe(
						Effect.mapError(() => destination_error("invalid_destination")),
					);

					if (!path_service.isAbsolute(destination_path)) {
						return yield* Effect.fail(destination_error("invalid_destination"));
					}

					const requested_destination = path_service.resolve(destination_path);
					const destination = yield* ReadDirectory(
						requested_destination,
						"destination_unavailable",
					);
					const canonical_parent = yield* file_system
						.realPath(path_service.dirname(destination.canonical_path))
						.pipe(Effect.mapError(() => destination_error("invalid_destination")));

					if (
						!same_canonical_path(
							path_service,
							requested_destination,
							destination.canonical_path,
						) ||
						!same_canonical_path(path_service, canonical_parent, projects_root) ||
						!same_canonical_path(
							path_service,
							path_service.dirname(destination.canonical_path),
							canonical_parent,
						)
					) {
						return yield* Effect.fail(destination_error("invalid_destination"));
					}

					const entries = require_empty
						? yield* file_system
								.readDirectory(destination.canonical_path)
								.pipe(
									Effect.mapError(() =>
										destination_error("destination_unavailable"),
									),
								)
						: [];

					if (entries.length > 0) {
						return yield* Effect.fail(destination_error("destination_not_empty"));
					}

					const current = yield* ReadDirectory(
						destination.canonical_path,
						"destination_unavailable",
					);

					if (
						!same_canonical_path(
							path_service,
							current.canonical_path,
							destination.canonical_path,
						) ||
						!same_file_identity(current.identity, destination.identity)
					) {
						return yield* Effect.fail(destination_error("destination_unavailable"));
					}

					return destination;
				});

			const Plan = (destination_path: unknown) =>
				Effect.scoped(
					Effect.gen(function* () {
						const projects_root = yield* ReadProjectsRoot;
						const destination = yield* ReadDestination(
							destination_path,
							projects_root.canonical_path,
							true,
						);
						const current_projects_root = yield* ReadProjectsRoot;

						if (
							!same_canonical_path(
								path_service,
								current_projects_root.canonical_path,
								projects_root.canonical_path,
							) ||
							!same_file_identity(
								current_projects_root.identity,
								projects_root.identity,
							)
						) {
							return yield* Effect.fail(
								destination_error("projects_root_unavailable"),
							);
						}

						return yield* Schema.decodeUnknownEffect(
							HostedProjectCloneDestinationPlan,
							{ onExcessProperty: "error" },
						)({
							canonical_root: destination.canonical_path,
							projects_root: projects_root.canonical_path,
							projects_root_device: projects_root.identity.device.toString(),
							projects_root_inode: projects_root.identity.inode.toString(),
							root_device: destination.identity.device.toString(),
							root_inode: destination.identity.inode.toString(),
						}).pipe(Effect.mapError(() => destination_error("invalid_destination")));
					}),
				);

			const VerifyPlan = (plan: HostedProjectCloneDestinationPlan, require_empty: boolean) =>
				Effect.gen(function* () {
					const decoded = yield* Schema.decodeUnknownEffect(
						HostedProjectCloneDestinationPlan,
						{ onExcessProperty: "error" },
					)(plan).pipe(Effect.mapError(() => destination_error("invalid_destination")));
					const projects_root = yield* ReadProjectsRoot;
					const destination = yield* ReadDestination(
						decoded.canonical_root,
						projects_root.canonical_path,
						require_empty,
					);
					const expected_projects_root = decode_identity(
						decoded.projects_root_device,
						decoded.projects_root_inode,
					);
					const expected_destination = decode_identity(
						decoded.root_device,
						decoded.root_inode,
					);

					if (
						!same_canonical_path(
							path_service,
							projects_root.canonical_path,
							decoded.projects_root,
						) ||
						!same_file_identity(projects_root.identity, expected_projects_root)
					) {
						return yield* Effect.fail(destination_error("projects_root_unavailable"));
					}

					if (!same_file_identity(destination.identity, expected_destination)) {
						return yield* Effect.fail(destination_error("destination_unavailable"));
					}

					return decoded;
				});

			const WithPinned = <A, E, R>(
				plan: HostedProjectCloneDestinationPlan,
				use: (destination: GitProviderCloneDestinationProofValue) => Effect.Effect<A, E, R>,
			) =>
				Effect.scoped(
					Effect.gen(function* () {
						const destination = yield* VerifyPlan(plan, true);
						const result = yield* use(destination);

						yield* VerifyPlan(destination, false);

						return result;
					}),
				);

			return { Plan, WithPinned };
		}),
	);
}
