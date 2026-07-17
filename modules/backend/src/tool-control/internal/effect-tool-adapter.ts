import { Cause, Data, Effect, Option, Schema, Stream } from "effect";
import { AiError, Tool, Toolkit } from "effect/unstable/ai";

import {
	type ToolDescriptorReference,
	type ToolInvocationContext,
	ToolArguments,
	ToolInputSchema,
	ToolResult,
} from "@artisan/protocol";

type ToolArgumentsValue = typeof ToolArguments.Type;
type ToolResultValue = typeof ToolResult.Type;

function execution_error(error: unknown) {
	return AiError.isAiError(error) &&
		error.reason instanceof AiError.UnknownError &&
		error.reason.description === "execution_failed"
		? new EffectToolAdapterError({ reason_code: "execution_failed" })
		: new EffectToolAdapterError({ reason_code: "invalid_result" });
}

function adapter_cause(cause: Cause.Cause<EffectToolAdapterError>) {
	if (Cause.hasInterrupts(cause)) {
		return Effect.failCause(cause);
	}

	const failure = Cause.findErrorOption(cause);

	return Effect.fail(
		Option.isSome(failure) && failure.value instanceof EffectToolAdapterError
			? failure.value
			: new EffectToolAdapterError({ reason_code: "execution_failed" }),
	);
}

/** Represents a source-safe failure while adapting one Effect AI tool. */
export class EffectToolAdapterError extends Data.TaggedError("EffectToolAdapterError")<{
	readonly reason_code: "execution_failed" | "invalid_arguments" | "invalid_result";
}> {}

/** Carries the durable invocation identity and canonical descriptor into an adapter. */
export interface EffectToolAdapterInvocation {
	readonly context: ToolInvocationContext;
	readonly invocation_id: string;
	readonly tool: ToolDescriptorReference;
}

/** Erases Effect AI implementation types after validating tool arguments and results. */
export interface EffectToolAdapter {
	readonly input_schema: typeof ToolInputSchema.Type;
	readonly Invoke: (
		invocation: EffectToolAdapterInvocation,
		arguments_: ToolArgumentsValue,
	) => Effect.Effect<ToolResultValue, EffectToolAdapterError>;
}

/** Builds an Effect AI-backed adapter and erases its generic schemas after validation. */
export function make_effect_tool_adapter<
	Parameters,
	ParametersEncoded,
	Success,
	SuccessEncoded,
>(input: {
	readonly handler: (
		invocation: EffectToolAdapterInvocation,
		parameters: Parameters,
	) => Effect.Effect<unknown, unknown>;
	readonly parameters: Schema.Codec<Parameters, ParametersEncoded>;
	readonly success: Schema.Codec<Success, SuccessEncoded>;
}): EffectToolAdapter {
	const ToolDefinition = Tool.make("tool", {
		parameters: ToolArguments,
		success: ToolResult,
	});
	const ToolkitDefinition = Toolkit.make(ToolDefinition);
	const input_schema = Schema.decodeUnknownSync(ToolInputSchema)(
		Tool.getJsonSchemaFromSchema(input.parameters),
	);

	const InvokeProgram = (
		invocation: EffectToolAdapterInvocation,
		arguments_: ToolArgumentsValue,
	) =>
		Effect.gen(function* () {
			const parameters = yield* Schema.decodeUnknownEffect(input.parameters, {
				onExcessProperty: "error",
			})(arguments_).pipe(
				Effect.mapError(
					() => new EffectToolAdapterError({ reason_code: "invalid_arguments" }),
				),
			);
			const toolkit = yield* ToolkitDefinition.pipe(
				Effect.provide(
					ToolkitDefinition.toLayer(
						ToolkitDefinition.of({
							tool: () =>
								input.handler(invocation, parameters).pipe(
									Effect.mapError(
										() =>
											new AiError.UnknownError({
												description: "execution_failed",
											}),
									),
									Effect.flatMap((result) =>
										Schema.decodeUnknownEffect(Schema.toType(input.success), {
											onExcessProperty: "error",
										})(result).pipe(
											Effect.mapError(
												() =>
													new AiError.InvalidToolResultError({
														description: "invalid_result",
														toolName: "tool",
													}),
											),
										),
									),
									Effect.flatMap((result) =>
										Schema.encodeUnknownEffect(input.success)(result).pipe(
											Effect.mapError(
												() =>
													new AiError.InvalidToolResultError({
														description: "invalid_result",
														toolName: "tool",
													}),
											),
										),
									),
									Effect.flatMap((result) =>
										Schema.decodeUnknownEffect(ToolResult)(result).pipe(
											Effect.mapError(
												() =>
													new AiError.InvalidToolResultError({
														description: "invalid_result",
														toolName: "tool",
													}),
											),
										),
									),
								),
						}),
					),
				),
			);
			const stream = yield* toolkit
				.handle("tool", arguments_)
				.pipe(Effect.mapError(execution_error));
			const handled = yield* Stream.runLast(stream).pipe(Effect.mapError(execution_error));

			if (Option.isNone(handled) || handled.value.preliminary) {
				return yield* Effect.fail(
					new EffectToolAdapterError({ reason_code: "invalid_result" }),
				);
			}

			return yield* Schema.decodeUnknownEffect(ToolResult)(handled.value.encodedResult).pipe(
				Effect.mapError(
					() => new EffectToolAdapterError({ reason_code: "invalid_result" }),
				),
			);
		});
	const Invoke = (invocation: EffectToolAdapterInvocation, arguments_: ToolArgumentsValue) =>
		InvokeProgram(invocation, arguments_).pipe(Effect.catchCause(adapter_cause));

	return {
		input_schema,
		Invoke,
	};
}
