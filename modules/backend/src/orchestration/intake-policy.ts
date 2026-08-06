import { Context, Effect, Layer, Schema } from "effect";

/** The explicit risk classification used by the harness before creating a run. */
export const IntakeRisk = Schema.Literals(["low", "material", "high", "underspecified"]);
export type IntakeRisk = typeof IntakeRisk.Type;

export const IntakeAssessment = Schema.Struct({
	risk: IntakeRisk,
	resolution: Schema.Literals(["proceed", "question"]),
	assumptions: Schema.Array(Schema.NonEmptyString),
	question: Schema.optional(Schema.NonEmptyString),
});
export type IntakeAssessment = typeof IntakeAssessment.Type;

/** Owns provider-neutral, validated pre-execution intake decisions. */
export class IntakePolicy extends Context.Service<
	IntakePolicy,
	{
		readonly Assess: (text: string) => Effect.Effect<IntakeAssessment>;
	}
>()("Artisan/IntakePolicy") {}

/**
 * The local policy never parks a message: keyword sniffing cannot judge the
 * risk of prose ("remove the emulator" is a feature request, not an
 * operation), and a wrong "question" resolution silently swallows the send.
 * Risk triage that can actually read the request replaces this Layer while
 * preserving the durable assessment contract.
 */
export const IntakePolicyLive = Layer.succeed(IntakePolicy, {
	Assess: () =>
		Effect.succeed({
			risk: "low" as const,
			resolution: "proceed" as const,
			assumptions: [],
		}),
});
