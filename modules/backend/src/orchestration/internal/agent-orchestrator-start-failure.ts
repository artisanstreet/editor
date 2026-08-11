import { Cause, Option } from "effect";

import { artisan_error_codes } from "@artisan/catalog";
import type { EngineErrorRef } from "@artisan/engines";

type StartFailureKind = "configuration" | "engine_error" | "interrupted" | "timeout";

export interface StartFailure {
	readonly diagnostic?: string;
	/** The failure in Artisan custody, when the cause could be classified. */
	readonly error_ref?: EngineErrorRef;
	readonly kind: StartFailureKind;
	readonly message: string;
}

const tagged_failure = (cause: Cause.Cause<unknown>) => {
	const failure = Cause.findErrorOption(cause);

	if (Option.isNone(failure)) return undefined;

	const value = failure.value;
	if (
		value === null ||
		typeof value !== "object" ||
		!("_tag" in value) ||
		typeof value._tag !== "string" ||
		!/^[A-Za-z][A-Za-z0-9_.-]{0,63}$/.test(value._tag)
	) {
		return undefined;
	}

	/**
	 * The engine's own message names the actual cause — a probe's stderr, an
	 * auth reason — so the settled failure must carry it; the tag alone reads
	 * as an internal fault and sends whoever hits it hunting the wrong layer.
	 */
	const message =
		"message" in value && typeof value.message === "string"
			? value.message.replaceAll(/\s+/gu, " ").trim().slice(0, 256)
			: "";
	/** An adapter that classified its own failure hands the code over verbatim. */
	const artisan_code =
		"artisan_code" in value && typeof value.artisan_code === "string"
			? value.artisan_code
			: undefined;

	return {
		artisan_code,
		detail: message.length === 0 ? undefined : message,
		name: value._tag,
	};
};

export const start_failure_from_cause = (cause: Cause.Cause<unknown>): StartFailure => {
	if (Cause.hasInterruptsOnly(cause)) {
		return {
			kind: "interrupted",
			message: "Engine startup was interrupted before the native session became ready.",
		};
	}

	const failure = tagged_failure(cause);

	return {
		diagnostic: Cause.pretty(cause),
		error_ref: {
			/**
			 * The adapter's own classification wins — it knows an auth-gated
			 * probe from a missing binary. Without one, the tag still separates
			 * "the engine said it is unavailable" from every other startup death.
			 */
			artisan_code:
				failure?.artisan_code ??
				(failure?.name === "EngineUnavailableError"
					? artisan_error_codes.engine_unavailable
					: artisan_error_codes.engine_start_failed),
			...(failure?.detail === undefined ? {} : { detail: failure.detail }),
			...(failure?.name === undefined ? {} : { provider_code: failure.name }),
		},
		kind: "engine_error",
		message:
			failure === undefined
				? "Engine startup failed before the native session became ready."
				: `Engine startup failed before the native session became ready (${failure.name}${failure.detail === undefined ? "" : `: ${failure.detail}`}).`,
	};
};
