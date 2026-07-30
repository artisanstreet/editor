import { Context, Effect, Layer, Option } from "effect";

import { SnowflakeId } from "@artisan/protocol";

import { backup_name, type CanonicalContent, ContentMetadata } from "./content";
import {
	GlobalGuidanceInvariantError,
	type GlobalGuidanceServiceError,
	type GlobalGuidanceServiceOptions,
} from "./contracts";
import { GuidanceFileStore } from "./file-store";
import { GlobalGuidanceRepository } from "./repository";
import { provider_mutation_fingerprint, type PreparedProviderMutation } from "./content";

export class GuidanceCanonical extends Context.Service<
	GuidanceCanonical,
	{
		readonly PrepareProviderMutation: (
			operation_id: string,
			make_request: (canonical_hash: string) => Readonly<Record<string, unknown>>,
		) => Effect.Effect<PreparedProviderMutation, GlobalGuidanceServiceError, never>;
		readonly Read: Effect.Effect<Option.Option<CanonicalContent>, GlobalGuidanceServiceError>;
		readonly Verify: (
			path: string,
			expected: CanonicalContent,
		) => Effect.Effect<CanonicalContent, GlobalGuidanceServiceError>;
		readonly Write: (
			next: CanonicalContent,
			label: string,
		) => Effect.Effect<
			CanonicalContent,
			GlobalGuidanceInvariantError | import("./file-store").GuidanceFileStoreFailure
		>;
	}
>()("Artisan/GuidanceCanonical") {}

export const make_guidance_canonical_layer = (options: GlobalGuidanceServiceOptions) =>
	Layer.effect(
		GuidanceCanonical,
		Effect.gen(function* () {
			const files = yield* GuidanceFileStore;
			const repository = yield* GlobalGuidanceRepository;
			const snowflake_id = yield* SnowflakeId;

			const Read = files.Read(options.canonical_path).pipe(
				Effect.flatMap(
					Option.match({
						onNone: () => Effect.succeed(Option.none<CanonicalContent>()),
						onSome: (file) =>
							ContentMetadata(file.content).pipe(Effect.map(Option.some)),
					}),
				),
			);

			const Verify = (path: string, expected: CanonicalContent) =>
				files.Read(path).pipe(
					Effect.flatMap(
						Option.match({
							onNone: () =>
								Effect.fail(
									new GlobalGuidanceInvariantError({
										operation: "post_write_file_missing",
									}),
								),
							onSome: (file) =>
								ContentMetadata(file.content).pipe(
									Effect.flatMap((actual) =>
										actual.content_hash === expected.content_hash
											? Effect.succeed(actual)
											: Effect.fail(
													new GlobalGuidanceInvariantError({
														operation: "post_write_hash_mismatch",
													}),
												),
									),
								),
						}),
					),
				);

			const Write = (next: CanonicalContent, label: string) =>
				Effect.gen(function* () {
					const observed = yield* Read;
					if (
						Option.isSome(observed) &&
						observed.value.content_hash === next.content_hash
					) {
						return yield* Verify(options.canonical_path, next);
					}
					const replacement = yield* files.ReplaceAtomic({
						backup_name: backup_name(
							label,
							options.canonical_path,
							yield* snowflake_id.Make("backup"),
						),
						backups_directory: options.backups_directory,
						content: next.content,
						...(Option.isSome(observed)
							? { expected_hash: observed.value.content_hash }
							: {}),
						path: options.canonical_path,
					});
					if (replacement._tag === "Changed") {
						return yield* new GlobalGuidanceInvariantError({
							operation: "canonical_changed_during_write",
						});
					}
					return yield* Verify(options.canonical_path, next);
				});

			const PrepareProviderMutation = (
				operation_id: string,
				make_request: (canonical_hash: string) => Readonly<Record<string, unknown>>,
			) =>
				Effect.gen(function* () {
					const canonical = yield* Read;
					if (Option.isNone(canonical)) {
						return yield* new GlobalGuidanceInvariantError({
							operation: "canonical_file_missing",
						});
					}
					const request_fingerprint = provider_mutation_fingerprint(
						make_request(canonical.value.content_hash),
					);
					const acceptance = yield* repository.PreflightProviderMutation({
						operation_id,
						request_fingerprint,
					});
					return {
						acceptance,
						canonical: canonical.value,
						request_fingerprint,
					} satisfies PreparedProviderMutation;
				});

			return { PrepareProviderMutation, Read, Verify, Write };
		}),
	);
