import { Cause, Context, Data, Effect, Layer, Option, Schema } from "effect";

import {
	type ListEligibleResult,
	type ToolDescriptor,
	type ToolDescriptorReference,
	type ToolInvocationContext,
	type ToolReasonCode,
	ToolArguments,
	ToolDescriptor as ToolDescriptorSchema,
	ToolInputSchema,
	ToolReasonCode as ToolReasonCodeSchema,
	ListEligibleResult as ListEligibleResultSchema,
	ToolDescriptorReference as ToolDescriptorReferenceSchema,
	ToolInvocationContext as ToolInvocationContextSchema,
} from "@artisan/protocol";

import { type EffectToolAdapter, EffectToolAdapterError } from "./internal/effect-tool-adapter";

type ToolArgumentsValue = typeof ToolArguments.Type;
type ToolResultValue = typeof import("@artisan/protocol").ToolResult.Type;

const ToolRegistryReasonCode = Schema.Literals([
	"context_ineligible",
	"duplicate_registration",
	"execution_failed",
	"invalid_arguments",
	"invalid_registration",
	"invalid_result",
	"revision_mismatch",
	"tool_unavailable",
]);

/** Identifies the stable source-safe errors exposed by the canonical tool registry. */
export type ToolRegistryReasonCode = typeof ToolRegistryReasonCode.Type;

/** Reports a source-safe registry, eligibility, or execution failure. */
export class ToolRegistryError extends Data.TaggedError("ToolRegistryError")<{
	readonly reason_code: ToolRegistryReasonCode;
}> {}

/** Reports a source-safe eligibility result owned by an individual tool boundary. */
export class ToolIneligible extends Data.TaggedError("ToolIneligible")<{
	readonly reason_code: ToolReasonCode;
}> {}

/** Defines one immutable registration accepted when the registry Layer starts. */
export interface ToolRegistration {
	readonly adapter: EffectToolAdapter;
	readonly descriptor: ToolDescriptor;
	readonly IsEligible: (context: ToolInvocationContext) => Effect.Effect<void, ToolIneligible>;
}

/** Owns the immutable canonical Tool Control Plane registrations. */
export class ToolRegistry extends Context.Service<
	ToolRegistry,
	{
		readonly Invoke: (
			tool: ToolDescriptorReference,
			context: ToolInvocationContext,
			arguments_: ToolArgumentsValue,
		) => Effect.Effect<ToolResultValue, ToolRegistryError>;
		readonly List: (context: unknown) => Effect.Effect<ListEligibleResult, ToolRegistryError>;
		readonly Resolve: (
			tool: ToolDescriptorReference,
		) => Effect.Effect<ToolDescriptor, ToolRegistryError>;
	}
>()("Artisan/ToolRegistry") {}

function matches_reference(descriptor: ToolDescriptor, reference: ToolDescriptorReference) {
	return descriptor.tool_id === reference.tool_id && descriptor.revision === reference.revision;
}

type JsonObjectValue = { readonly [key: string]: Schema.Json };

function is_json_object(value: Schema.Json): value is JsonObjectValue {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

function freeze_json(value: Schema.Json): Schema.Json {
	if (Array.isArray(value)) {
		return Object.freeze(value.map(freeze_json));
	}

	if (!is_json_object(value)) {
		return value;
	}

	return Object.freeze(
		Object.fromEntries(
			Object.keys(value)
				.toSorted()
				.map((key) => [key, freeze_json(value[key]!)]),
		),
	);
}

function schemas_match(left: Schema.Json, right: Schema.Json) {
	return JSON.stringify(freeze_json(left)) === JSON.stringify(freeze_json(right));
}

function adapter_error(error: EffectToolAdapterError) {
	return new ToolRegistryError({ reason_code: error.reason_code });
}

function eligibility_reason(cause: Cause.Cause<unknown>): ToolReasonCode {
	const failure = Cause.findErrorOption(cause);
	const reason =
		Option.isSome(failure) && failure.value instanceof ToolIneligible
			? Schema.decodeUnknownOption(ToolReasonCodeSchema)(failure.value.reason_code)
			: Option.none<ToolReasonCode>();

	return Option.getOrElse(reason, () => "tool_unavailable" as const);
}

function CheckEligibility(registration: ToolRegistration, context: ToolInvocationContext) {
	return Effect.sync(() => registration.IsEligible(context)).pipe(
		Effect.flatMap((result) =>
			Effect.isEffect(result)
				? result
				: Effect.fail(new ToolIneligible({ reason_code: "tool_unavailable" })),
		),
		Effect.catchCause((cause) =>
			Cause.hasInterrupts(cause)
				? Effect.failCause(cause)
				: Effect.fail(new ToolIneligible({ reason_code: eligibility_reason(cause) })),
		),
	);
}

interface ToolRegistrationCandidate {
	readonly adapter: EffectToolAdapter;
	readonly descriptor: unknown;
	readonly IsEligible: ToolRegistration["IsEligible"];
}

function is_tool_registration_candidate(value: {
	readonly adapter: unknown;
	readonly descriptor: unknown;
	readonly IsEligible: unknown;
}): value is ToolRegistrationCandidate {
	return (
		typeof value.IsEligible === "function" &&
		typeof value.adapter === "object" &&
		value.adapter !== null &&
		"input_schema" in value.adapter &&
		"Invoke" in value.adapter &&
		typeof value.adapter.Invoke === "function"
	);
}

function BuildToolRegistry(registrations: ReadonlyArray<unknown>) {
	return Effect.gen(function* () {
		const decoded = yield* Effect.forEach(registrations, (registration) =>
			Effect.gen(function* () {
				const candidate = yield* Schema.decodeUnknownEffect(
					Schema.Struct({
						adapter: Schema.Unknown,
						descriptor: Schema.Unknown,
						IsEligible: Schema.Unknown,
					}),
					{ onExcessProperty: "error" },
				)(registration).pipe(
					Effect.mapError(
						() => new ToolRegistryError({ reason_code: "invalid_registration" }),
					),
				);

				if (!is_tool_registration_candidate(candidate)) {
					return yield* new ToolRegistryError({ reason_code: "invalid_registration" });
				}

				const descriptor = yield* Schema.decodeUnknownEffect(ToolDescriptorSchema, {
					onExcessProperty: "error",
				})(candidate.descriptor).pipe(
					Effect.mapError(
						() => new ToolRegistryError({ reason_code: "invalid_registration" }),
					),
				);
				const input_schema = yield* Schema.decodeUnknownEffect(ToolInputSchema)(
					candidate.adapter.input_schema,
				).pipe(
					Effect.mapError(
						() => new ToolRegistryError({ reason_code: "invalid_registration" }),
					),
				);

				if (!schemas_match(descriptor.input_schema, input_schema)) {
					return yield* Effect.fail(
						new ToolRegistryError({ reason_code: "invalid_registration" }),
					);
				}

				const frozen_input_schema = freeze_json(input_schema);
				const frozen_descriptor = Object.freeze({
					...descriptor,
					input_schema: frozen_input_schema,
				});
				const frozen_adapter = Object.freeze({
					input_schema: frozen_input_schema,
					Invoke: candidate.adapter.Invoke,
				});

				return Object.freeze({
					adapter: frozen_adapter,
					descriptor: frozen_descriptor,
					IsEligible: candidate.IsEligible,
				}) satisfies ToolRegistration;
			}),
		);
		const tool_ids = decoded.map((registration) => registration.descriptor.tool_id);

		if (new Set(tool_ids).size !== tool_ids.length) {
			return yield* Effect.fail(
				new ToolRegistryError({ reason_code: "duplicate_registration" }),
			);
		}

		const sorted = decoded.toSorted((left, right) =>
			left.descriptor.tool_id.localeCompare(right.descriptor.tool_id),
		);
		const by_tool_id = new Map(
			sorted.map((registration) => [registration.descriptor.tool_id, registration]),
		);
		const Resolve = (reference: ToolDescriptorReference) => {
			return Schema.decodeUnknownEffect(ToolDescriptorReferenceSchema, {
				onExcessProperty: "error",
			})(reference).pipe(
				Effect.mapError(() => new ToolRegistryError({ reason_code: "tool_unavailable" })),
				Effect.flatMap((decoded_reference) => {
					const registration = by_tool_id.get(decoded_reference.tool_id);

					if (registration === undefined) {
						return Effect.fail(
							new ToolRegistryError({ reason_code: "tool_unavailable" }),
						);
					}

					return matches_reference(registration.descriptor, decoded_reference)
						? Effect.succeed(registration.descriptor)
						: Effect.fail(new ToolRegistryError({ reason_code: "revision_mismatch" }));
				}),
			);
		};
		const List = (context: unknown) =>
			Schema.decodeUnknownEffect(ToolInvocationContextSchema, {
				onExcessProperty: "error",
			})(context).pipe(
				Effect.mapError(() => new ToolRegistryError({ reason_code: "context_ineligible" })),
				Effect.flatMap((decoded_context) =>
					Effect.forEach(sorted, (registration) =>
						CheckEligibility(registration, decoded_context).pipe(
							Effect.as({
								descriptor: registration.descriptor,
								state: "eligible" as const,
							}),
							Effect.catch((error) =>
								Effect.succeed({
									descriptor: registration.descriptor,
									reason_code: error.reason_code,
									state: "unavailable" as const,
								}),
							),
						),
					),
				),
				Effect.flatMap((tools) =>
					Schema.decodeUnknownEffect(ListEligibleResultSchema, {
						onExcessProperty: "error",
					})({ tools }).pipe(
						Effect.mapError(
							() => new ToolRegistryError({ reason_code: "invalid_registration" }),
						),
					),
				),
			);
		const Invoke = (
			reference: ToolDescriptorReference,
			context: ToolInvocationContext,
			arguments_: ToolArgumentsValue,
		) =>
			Effect.gen(function* () {
				const decoded_context = yield* Schema.decodeUnknownEffect(
					ToolInvocationContextSchema,
					{ onExcessProperty: "error" },
				)(context).pipe(
					Effect.mapError(
						() => new ToolRegistryError({ reason_code: "context_ineligible" }),
					),
				);
				const decoded_arguments = yield* Schema.decodeUnknownEffect(ToolArguments, {
					onExcessProperty: "error",
				})(arguments_).pipe(
					Effect.mapError(
						() => new ToolRegistryError({ reason_code: "invalid_arguments" }),
					),
				);
				const descriptor = yield* Resolve(reference);
				const registration = by_tool_id.get(descriptor.tool_id);

				if (registration === undefined) {
					return yield* Effect.fail(
						new ToolRegistryError({ reason_code: "tool_unavailable" }),
					);
				}

				yield* CheckEligibility(registration, decoded_context).pipe(
					Effect.mapError(
						() => new ToolRegistryError({ reason_code: "context_ineligible" }),
					),
				);

				return yield* registration.adapter
					.Invoke(decoded_context, decoded_arguments)
					.pipe(Effect.mapError(adapter_error));
			});

		return { Invoke, List, Resolve };
	});
}

/** Builds a closed immutable registry; registrations cannot be added after Layer construction. */
export function make_tool_registry_layer(registrations: ReadonlyArray<unknown>) {
	return Layer.effect(ToolRegistry, BuildToolRegistry(registrations));
}
