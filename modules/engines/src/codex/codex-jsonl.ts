import { Buffer } from "node:buffer";

import { Data } from "effect";

/** Represents one malformed UTF-8 or JSON line from the Codex stdio transport. @since 0.3.0 */
export class CodexJsonlMalformedLineError extends Data.TaggedError("CodexJsonlMalformedLineError")<{
	readonly line_base64: string;
	readonly message: string;
	readonly raw_frame_base64: string;
}> {}

/** Preserves a decoded JSONL payload together with its byte-exact frame provenance. @since 0.3.0 */
export interface CodexJsonlFrame {
	readonly line_base64: string;
	readonly payload: unknown;
	readonly raw_frame_base64: string;
}

/** Represents either a decoded JSONL frame or one recoverable malformed frame. @since 0.3.0 */
export type CodexJsonlDecode = CodexJsonlFrame | CodexJsonlMalformedLineError;

function concat_bytes(left: Uint8Array, right: Uint8Array) {
	const output = new Uint8Array(left.length + right.length);

	output.set(left);
	output.set(right, left.length);

	return output;
}

function decode_line(raw_frame: Uint8Array): CodexJsonlDecode {
	const normalized = raw_frame.at(-1) === 13 ? raw_frame.subarray(0, -1) : raw_frame;
	const line_base64 = Buffer.from(normalized).toString("base64");
	const raw_frame_base64 = Buffer.from(raw_frame).toString("base64");

	try {
		return {
			line_base64,
			payload: JSON.parse(
				new TextDecoder("utf-8", { fatal: true }).decode(normalized),
			) as unknown,
			raw_frame_base64,
		};
	} catch (cause) {
		const message = cause instanceof Error ? cause.message : "Unknown JSONL decoding error";

		return new CodexJsonlMalformedLineError({ line_base64, message, raw_frame_base64 });
	}
}

function is_malformed_line(value: CodexJsonlDecode): value is CodexJsonlMalformedLineError {
	return value instanceof CodexJsonlMalformedLineError;
}

/** Incrementally frames UTF-8 JSONL without decoding a partial byte sequence. @since 0.1.0 */
export class CodexJsonlFramer {
	#pending = new Uint8Array();

	/**
	 * Accepts a process chunk and emits every complete frame, retaining malformed
	 * frames so callers can diagnose them and continue with subsequent messages.
	 *
	 * @since 0.3.0
	 * @param chunk - Raw bytes received from the process stdout stream.
	 * @returns Decoded frames and recoverable malformed-frame diagnostics.
	 */
	PushRecovering(chunk: Uint8Array): ReadonlyArray<CodexJsonlDecode> {
		const bytes = concat_bytes(this.#pending, chunk);
		const values: Array<CodexJsonlDecode> = [];
		let start = 0;

		for (let index = 0; index < bytes.length; index += 1) {
			if (bytes[index] !== 10) {
				continue;
			}

			values.push(decode_line(bytes.subarray(start, index)));
			start = index + 1;
		}

		this.#pending = bytes.subarray(start);

		return values;
	}

	/**
	 * Decodes final unterminated data while retaining malformed-frame diagnostics.
	 *
	 * @since 0.3.0
	 * @returns The final frame or an empty list when no bytes remain.
	 */
	FinishRecovering(): ReadonlyArray<CodexJsonlDecode> {
		if (this.#pending.length === 0) {
			return [];
		}

		const values = [decode_line(this.#pending)];

		this.#pending = new Uint8Array();

		return values;
	}

	/**
	 * Accepts a process chunk and emits each complete JSON value it contains.
	 *
	 * @since 0.1.0
	 * @param chunk - Raw bytes received from the process stdout stream.
	 * @returns The decoded values terminated by newline bytes in this chunk.
	 */
	Push(chunk: Uint8Array): ReadonlyArray<unknown> {
		const values = this.PushRecovering(chunk);
		const malformed = values.find(is_malformed_line);

		if (malformed) {
			throw malformed;
		}

		return values.map((value) => {
			if (is_malformed_line(value)) {
				throw value;
			}

			return value.payload;
		});
	}

	/**
	 * Decodes the final unterminated JSON value when the stream closes.
	 *
	 * @since 0.1.0
	 * @returns The final value, or an empty list when no bytes remain.
	 */
	Finish(): ReadonlyArray<unknown> {
		const values = this.FinishRecovering();
		const malformed = values.find(is_malformed_line);

		if (malformed) {
			throw malformed;
		}

		return values.map((value) => {
			if (is_malformed_line(value)) {
				throw value;
			}

			return value.payload;
		});
	}
}
