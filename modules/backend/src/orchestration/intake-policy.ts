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

const ambiguous_request = /\b(it|that|this|something|whatever)\b/i;
const high_risk_request =
	/\b(delete|drop\s+table|reset\s+--hard|production|deploy|publish|payment|credential|secret)\b/i;
const material_request = /\b(migrate|database|security|permission|overwrite|remove|release)\b/i;

/**
 * Conservative local policy for the Codex-only prototype. It deliberately does
 * not add a second provider integration; callers can replace this Layer with a
 * richer policy while preserving the durable assessment contract.
 */
export const IntakePolicyLive = Layer.succeed(IntakePolicy, {
	Assess: (text) =>
		Effect.sync(() => {
			const normalized = text.trim();

			if (/^(help|fix|implement)$/i.test(normalized)) {
				return {
					risk: "underspecified" as const,
					resolution: "question" as const,
					assumptions: [],
					question: "What outcome, scope, and constraints should Artisan use?",
				};
			}

			if (high_risk_request.test(normalized)) {
				return {
					risk: "high" as const,
					resolution: "question" as const,
					assumptions: [],
					question:
						"This request may have high-impact consequences. Please confirm the intended scope.",
				};
			}

			if (material_request.test(normalized)) {
				return {
					risk: "material" as const,
					resolution: "question" as const,
					assumptions: [],
					question: "Which exact scope and safeguards should Artisan apply?",
				};
			}

			return ambiguous_request.test(normalized)
				? {
						risk: "low" as const,
						resolution: "proceed" as const,
						assumptions: [
							"Proceeding with the current thread context for the ambiguous reference.",
						],
					}
				: { risk: "low" as const, resolution: "proceed" as const, assumptions: [] };
		}),
});
