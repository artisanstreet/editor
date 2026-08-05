import type { SurfaceUsageAggregate } from "@artisan/protocol";

/**
 * Where each engine starts compacting, as a percentage of the same nominal
 * context window the gauge divides by.
 *
 * The number is a property of the harness and the model, not of Artisan, so it
 * is read from what each engine documents rather than estimated from observed
 * behaviour. An engine whose behaviour is not documented reports the only bound
 * that is certainly true — the window itself.
 */

/**
 * Codex clamps compaction to nine tenths of the resolved context window and
 * silently lowers any larger `model_auto_compact_token_limit` to that ceiling,
 * so this is a hard limit rather than a tendency:
 *
 * ```rust
 * // codex-rs/protocol/src/openai_models.rs
 * let context_limit = self.resolved_context_window()
 *     .map(|context_window| (context_window * 9) / 10);
 * config_limit.map_or(context_limit, |limit| std::cmp::min(limit, context_limit))
 * ```
 */
const codex_compaction_percent = 90;

/**
 * Claude Code sizes compaction by a token capacity rather than a fraction.
 * `CLAUDE_CODE_AUTO_COMPACT_WINDOW` "defaults to the model's context window,
 * 200K for standard models or 1M for extended context models", which puts
 * compaction at the window itself — except on Sonnet 5, which "auto-compact[s]
 * before the window fills, at about 967K tokens by default".
 *
 * Capping the capacity at the window is what the documentation describes rather
 * than an extra safeguard: a Sonnet 5 session budgeted at 200K by an LLM gateway
 * or `CLAUDE_CODE_DISABLE_1M_CONTEXT` compacts at that smaller boundary.
 */
const claude_sonnet_5_compaction_tokens = 967_000;

const claude_compaction_tokens = (native_model_id: string | undefined, window_tokens: number) =>
	native_model_id === "claude-sonnet-5"
		? Math.min(claude_sonnet_5_compaction_tokens, window_tokens)
		: window_tokens;

/** Identifies one model precisely enough to know when its engine compacts. */
export interface AutoCompactionModel {
	readonly harness: string;
	readonly native_model_id: string | undefined;
	/** The denominator the gauge uses, which is what the percentage is against. */
	readonly window_tokens: number;
}

/**
 * Returns the percentage at which this model's engine begins compacting.
 *
 * Undocumented engines fall back to 100: the window filling is the only claim
 * that holds without a source, and overstating it would make the gauge cry
 * wolf on an engine that may never compact at all.
 */
export const ContextAutoCompactionPercent = (model: AutoCompactionModel) => {
	if (model.window_tokens <= 0) {
		return 100;
	}

	if (model.harness === "codex") {
		return codex_compaction_percent;
	}

	if (model.harness === "claude") {
		return Math.min(
			100,
			(claude_compaction_tokens(model.native_model_id, model.window_tokens) /
				model.window_tokens) *
				100,
		);
	}

	return 100;
};

/**
 * Resolves the threshold for the run that reported a context gauge. A thread's
 * current policy describes its next launch, and therefore must not reinterpret
 * an already-reported window. Incomplete run provenance is intentionally the
 * unknown-engine case, whose truthful threshold is 100%.
 */
export const ContextUsageAutoCompactionPercent = (
	usage: SurfaceUsageAggregate | undefined,
	window_tokens: number,
) =>
	ContextAutoCompactionPercent({
		harness: usage?.context_origin?.engine_id ?? "",
		native_model_id: usage?.context_origin?.model_id,
		window_tokens,
	});
