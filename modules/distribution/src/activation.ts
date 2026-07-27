import { Context, Data, Effect, FileSystem, Layer, Path, Schema, Semaphore } from "effect";

import { SemanticVersion, type SemanticVersion as SemanticVersionValue } from "./common";

export const ActiveVersionPointer = Schema.Struct({
	format_version: Schema.Literal(1),
	active_version: SemanticVersion,
	previous_version: Schema.optional(SemanticVersion),
});
export type ActiveVersionPointer = typeof ActiveVersionPointer.Type;

export type ActiveVersionState =
	| { readonly _tag: "Absent" }
	| { readonly _tag: "Active"; readonly pointer: ActiveVersionPointer }
	| { readonly _tag: "Malformed"; readonly cause: unknown; readonly pointer_path: string };

export class ActivationFailure extends Data.TaggedError("ActivationFailure")<{
	readonly cause: unknown;
	readonly operation: "activate" | "deactivate" | "layout" | "read" | "rollback";
	readonly path: string;
}> {}

export class ActivationInvalidVersion extends Data.TaggedError("ActivationInvalidVersion")<{
	readonly cause: unknown;
	readonly version: string;
}> {}

export class ActivationVersionUnavailable extends Data.TaggedError("ActivationVersionUnavailable")<{
	readonly version: SemanticVersionValue;
	readonly version_path: string;
}> {}

export class ActivationRollbackUnavailable extends Data.TaggedError(
	"ActivationRollbackUnavailable",
)<{}> {}

export interface ManagedInstallationLayout {
	readonly bin: string;
	readonly current: string;
	readonly root: string;
	readonly staging: string;
	readonly versions: string;
}

export class Activation extends Context.Service<
	Activation,
	{
		readonly Activate: (
			version: string,
		) => Effect.Effect<
			ActiveVersionPointer,
			ActivationFailure | ActivationInvalidVersion | ActivationVersionUnavailable
		>;
		/** Removes the active pointer without removing a staged version. */
		readonly Deactivate: () => Effect.Effect<void, ActivationFailure>;
		readonly EnsureLayout: () => Effect.Effect<ManagedInstallationLayout, ActivationFailure>;
		readonly ReadActive: () => Effect.Effect<ActiveVersionState, ActivationFailure>;
		readonly Rollback: () => Effect.Effect<
			ActiveVersionPointer,
			ActivationFailure | ActivationRollbackUnavailable | ActivationVersionUnavailable
		>;
	}
>()("Artisan/Distribution/Activation") {}

const is_not_found = (cause: unknown) =>
	typeof cause === "object" &&
	cause !== null &&
	"reason" in cause &&
	typeof cause.reason === "object" &&
	cause.reason !== null &&
	"_tag" in cause.reason &&
	cause.reason._tag === "NotFound";

/** Supplies the managed binary layout and atomic active-version pointer. */
export const make_activation_layer = (installation_root: string) =>
	Layer.effect(
		Activation,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			const path_service = yield* Path.Path;
			const mutation_lock = yield* Semaphore.make(1);
			const layout: ManagedInstallationLayout = {
				root: installation_root,
				bin: path_service.join(installation_root, "bin"),
				versions: path_service.join(installation_root, "versions"),
				staging: path_service.join(installation_root, "staging"),
				current: path_service.join(installation_root, "current"),
			};
			const temporary_pointer = path_service.join(installation_root, ".current.tmp");

			const EnsureLayout = () =>
				Effect.forEach([layout.bin, layout.versions, layout.staging], (directory) =>
					file_system.makeDirectory(directory, { recursive: true }),
				).pipe(
					Effect.as(layout),
					Effect.mapError(
						(cause) =>
							new ActivationFailure({
								cause,
								operation: "layout",
								path: installation_root,
							}),
					),
				);

			const ReadActive = () =>
				file_system.readFileString(layout.current).pipe(
					Effect.flatMap((content) =>
						Effect.try({
							try: () => JSON.parse(content) as unknown,
							catch: (cause) => cause,
						}).pipe(
							Effect.flatMap(Schema.decodeUnknownEffect(ActiveVersionPointer)),
							Effect.match({
								onFailure: (cause): ActiveVersionState => ({
									_tag: "Malformed",
									cause,
									pointer_path: layout.current,
								}),
								onSuccess: (pointer): ActiveVersionState => ({
									_tag: "Active",
									pointer,
								}),
							}),
						),
					),
					Effect.catch((cause) =>
						is_not_found(cause)
							? Effect.succeed<ActiveVersionState>({ _tag: "Absent" })
							: Effect.fail(
									new ActivationFailure({
										cause,
										operation: "read",
										path: layout.current,
									}),
								),
					),
				);

			const ValidateVersion = (version: string) =>
				Schema.decodeUnknownEffect(SemanticVersion)(version).pipe(
					Effect.mapError((cause) => new ActivationInvalidVersion({ cause, version })),
				);

			const AssertVersionAvailable = (version: SemanticVersionValue) =>
				Effect.gen(function* () {
					const version_path = path_service.join(layout.versions, version);
					const metadata = yield* file_system.stat(version_path).pipe(
						Effect.mapError((cause) =>
							is_not_found(cause)
								? new ActivationVersionUnavailable({ version, version_path })
								: new ActivationFailure({
										cause,
										operation: "activate",
										path: version_path,
									}),
						),
					);

					if (metadata.type !== "Directory") {
						return yield* new ActivationVersionUnavailable({ version, version_path });
					}
				});

			const Publish = (
				pointer: ActiveVersionPointer,
				operation: ActivationFailure["operation"],
			) =>
				Effect.gen(function* () {
					yield* EnsureLayout();
					yield* Effect.scoped(
						Effect.gen(function* () {
							const file = yield* file_system.open(temporary_pointer, { flag: "w" });
							yield* file.writeAll(
								new TextEncoder().encode(
									`${JSON.stringify(pointer, undefined, "\t")}\n`,
								),
							);
							yield* file.sync;
						}),
					);
					yield* file_system.rename(temporary_pointer, layout.current);
					return pointer;
				}).pipe(
					Effect.ensuring(file_system.remove(temporary_pointer).pipe(Effect.ignore)),
					Effect.mapError((cause) =>
						cause instanceof ActivationFailure
							? cause
							: new ActivationFailure({ cause, operation, path: layout.current }),
					),
				);

			const Activate = (version: string) =>
				mutation_lock.withPermits(1)(
					Effect.gen(function* () {
						const validated_version = yield* ValidateVersion(version);
						yield* AssertVersionAvailable(validated_version);
						const current = yield* ReadActive();
						const previous_version =
							current._tag === "Active" ? current.pointer.active_version : undefined;

						return yield* Publish(
							{
								format_version: 1,
								active_version: validated_version,
								...(previous_version === undefined ? {} : { previous_version }),
							},
							"activate",
						);
					}),
				);

			const Rollback = () =>
				mutation_lock.withPermits(1)(
					Effect.gen(function* () {
						const current = yield* ReadActive();

						if (
							current._tag !== "Active" ||
							current.pointer.previous_version === undefined
						) {
							return yield* new ActivationRollbackUnavailable();
						}

						yield* AssertVersionAvailable(current.pointer.previous_version);
						return yield* Publish(
							{
								format_version: 1,
								active_version: current.pointer.previous_version,
								previous_version: current.pointer.active_version,
							},
							"rollback",
						);
					}),
				);

			const Deactivate = () =>
				mutation_lock.withPermits(1)(
					file_system.remove(layout.current).pipe(
						Effect.catch((cause) =>
							is_not_found(cause)
								? Effect.void
								: Effect.fail(
										new ActivationFailure({
											cause,
											operation: "deactivate",
											path: layout.current,
										}),
									),
						),
					),
				);

			return { Activate, Deactivate, EnsureLayout, ReadActive, Rollback };
		}),
	);
