import { createHash } from "node:crypto";

import { Context, Data, Effect, Layer, Option, Schema } from "effect";

import type { InstalledIntegrations, OwnedIntegration } from "./installation-manifest";

export const IntegrationKind = Schema.Literals([
	"ae_path",
	"protocol",
	"application_shortcut",
	"forge_logs_shortcut",
	"forge_start_shortcut",
	"uninstall_shortcut",
	"desktop_shortcut",
	"autostart",
]);
export type IntegrationKind = typeof IntegrationKind.Type;

export const IntegrationSpecification = Schema.Struct({
	kind: IntegrationKind,
	path: Schema.NonEmptyString,
	content: Schema.NonEmptyString,
});
export type IntegrationSpecification = typeof IntegrationSpecification.Type;

export type IntegrationState =
	| { readonly _tag: "Missing"; readonly kind: IntegrationKind; readonly path: string }
	| { readonly _tag: "Owned"; readonly kind: IntegrationKind; readonly path: string }
	| {
			readonly _tag: "Drifted";
			readonly actual_fingerprint: string;
			readonly expected_fingerprint: string;
			readonly kind: IntegrationKind;
			readonly path: string;
	  };

export type IntegrationRemoval =
	| { readonly _tag: "Removed"; readonly kind: IntegrationKind; readonly path: string }
	| {
			readonly _tag: "Preserved";
			readonly kind: IntegrationKind;
			readonly path: string;
			readonly reason: "missing" | "ownership_mismatch";
	  };

export class IntegrationError extends Data.TaggedError("IntegrationError")<{
	readonly cause?: unknown;
	readonly code: "inspect_failed" | "install_failed" | "ownership_conflict" | "remove_failed";
	readonly kind: IntegrationKind;
	readonly path: string;
}> {}

/** Platform storage returns exact canonical content so ownership can be proven. */
export class IntegrationPlatform extends Context.Service<
	IntegrationPlatform,
	{
		readonly Read: (
			kind: IntegrationKind,
			path: string,
		) => Effect.Effect<Option.Option<string>, IntegrationError>;
		readonly Remove: (
			kind: IntegrationKind,
			path: string,
		) => Effect.Effect<void, IntegrationError>;
		readonly Write: (
			kind: IntegrationKind,
			path: string,
			content: string,
		) => Effect.Effect<void, IntegrationError>;
	}
>()("Artisan/Distribution/IntegrationPlatform") {}

export class IntegrationLifecycle extends Context.Service<
	IntegrationLifecycle,
	{
		readonly Inspect: (
			records: InstalledIntegrations,
		) => Effect.Effect<ReadonlyArray<IntegrationState>, IntegrationError>;
		readonly Install: (
			specifications: ReadonlyArray<IntegrationSpecification>,
		) => Effect.Effect<InstalledIntegrations, IntegrationError>;
		readonly Repair: (
			specifications: ReadonlyArray<IntegrationSpecification>,
			records: InstalledIntegrations,
		) => Effect.Effect<InstalledIntegrations, IntegrationError>;
		readonly Uninstall: (
			records: InstalledIntegrations,
		) => Effect.Effect<ReadonlyArray<IntegrationRemoval>, IntegrationError>;
	}
>()("Artisan/Distribution/IntegrationLifecycle") {}

export const IntegrationFingerprint = (content: string) =>
	createHash("sha256").update(content, "utf8").digest("hex");

const integration_kinds: ReadonlyArray<IntegrationKind> = [
	"ae_path",
	"protocol",
	"application_shortcut",
	"forge_logs_shortcut",
	"forge_start_shortcut",
	"uninstall_shortcut",
	"desktop_shortcut",
	"autostart",
];

const RecordEntries = (records: InstalledIntegrations) =>
	integration_kinds.flatMap((kind) => {
		const record = records[kind];
		return record === undefined ? [] : [{ kind, record }];
	});

const ToRecords = (
	entries: ReadonlyArray<{
		readonly kind: IntegrationKind;
		readonly record: OwnedIntegration;
	}>,
): InstalledIntegrations =>
	Object.fromEntries(entries.map(({ kind, record }) => [kind, record])) as InstalledIntegrations;

const InspectRecord = (
	platform: IntegrationPlatform["Service"],
	kind: IntegrationKind,
	record: OwnedIntegration,
) =>
	platform.Read(kind, record.path).pipe(
		Effect.map(
			Option.match({
				onNone: (): IntegrationState => ({ _tag: "Missing", kind, path: record.path }),
				onSome: (content): IntegrationState => {
					const actual_fingerprint = IntegrationFingerprint(content);
					return actual_fingerprint === record.fingerprint
						? { _tag: "Owned", kind, path: record.path }
						: {
								_tag: "Drifted",
								actual_fingerprint,
								expected_fingerprint: record.fingerprint,
								kind,
								path: record.path,
							};
				},
			}),
		),
	);

export const IntegrationLifecycleLive = Layer.effect(
	IntegrationLifecycle,
	Effect.gen(function* () {
		const platform = yield* IntegrationPlatform;

		const InstallOne = (specification: IntegrationSpecification) =>
			Effect.gen(function* () {
				const existing = yield* platform.Read(specification.kind, specification.path);
				if (Option.isSome(existing))
					return {
						kind: specification.kind,
						record: {
							path: specification.path,
							fingerprint: IntegrationFingerprint(specification.content),
						},
					};
				yield* platform.Write(
					specification.kind,
					specification.path,
					specification.content,
				);
				return {
					kind: specification.kind,
					record: {
						path: specification.path,
						fingerprint: IntegrationFingerprint(specification.content),
					},
				};
			});

		return IntegrationLifecycle.of({
			Inspect: (records) =>
				Effect.forEach(
					RecordEntries(records),
					({ kind, record }) => InspectRecord(platform, kind, record),
					{ concurrency: 1 },
				),
			Install: (specifications) =>
				Effect.gen(function* () {
					yield* Effect.forEach(
						specifications,
						(specification) =>
							platform.Read(specification.kind, specification.path).pipe(
								Effect.filterOrFail(
									Option.match({
										onNone: () => true,
										onSome: (content) =>
											IntegrationFingerprint(content) ===
											IntegrationFingerprint(specification.content),
									}),
									() =>
										new IntegrationError({
											code: "ownership_conflict",
											kind: specification.kind,
											path: specification.path,
										}),
								),
							),
						{ concurrency: 1, discard: true },
					);
					return yield* Effect.forEach(specifications, InstallOne, {
						concurrency: 1,
					}).pipe(Effect.map(ToRecords));
				}),
			Repair: (specifications, records) =>
				Effect.gen(function* () {
					const specifications_by_kind = new Map(
						specifications.map((specification) => [specification.kind, specification]),
					);
					const repaired = yield* Effect.forEach(
						RecordEntries(records),
						({ kind, record }) =>
							Effect.gen(function* () {
								const state = yield* InspectRecord(platform, kind, record);
								if (state._tag !== "Missing") return { kind, record };
								const specification = specifications_by_kind.get(kind);
								if (
									specification === undefined ||
									specification.path !== record.path ||
									IntegrationFingerprint(specification.content) !==
										record.fingerprint
								)
									return { kind, record };
								return yield* InstallOne(specification);
							}),
						{ concurrency: 1 },
					);
					return ToRecords(repaired);
				}),
			Uninstall: (records) =>
				Effect.forEach(
					RecordEntries(records),
					({ kind, record }) =>
						Effect.gen(function* () {
							const state = yield* InspectRecord(platform, kind, record);
							if (state._tag === "Owned") {
								yield* platform.Remove(kind, record.path);
								return { _tag: "Removed", kind, path: record.path } as const;
							}
							return {
								_tag: "Preserved",
								kind,
								path: record.path,
								reason:
									state._tag === "Missing"
										? ("missing" as const)
										: ("ownership_mismatch" as const),
							} as const;
						}),
					{ concurrency: 1 },
				),
		});
	}),
);
