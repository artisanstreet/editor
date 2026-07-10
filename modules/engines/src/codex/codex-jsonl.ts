import { Buffer } from "node:buffer";

import { Data } from "effect";

/** Represents one malformed UTF-8 or JSON line from the Codex stdio transport. @since 0.1.0 */
export class CodexJsonlMalformedLineError extends Data.TaggedError("CodexJsonlMalformedLineError")<{
	readonly line_base64: string;
	readonly message: string;
}> {}

function concat_bytes(left: Uint8Array, right: Uint8Array) {
	const output = new Uint8Array(left.length + right.length);

	output.set(left);
	output.set(right, left.length);

	return output;
}

function decode_line(line: Uint8Array) {
	const normalized = line.at(-1) === 13 ? line.subarray(0, -1) : line;
	const line_base64 = Buffer.from(normalized).toString("base64");

	try {
		return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(normalized)) as unknown;
	} catch (cause) {
		const message = cause instanceof Error ? cause.message : "Unknown JSONL decoding error";

		throw new CodexJsonlMalformedLineError({ line_base64, message });
	}
}

/** Incrementally frames UTF-8 JSONL without decoding a partial byte sequence. @since 0.1.0 */
export class CodexJsonlFramer {
	#pending = new Uint8Array();

	/**
	 * Accepts a process chunk and emits each complete JSON value it contains.
	 *
	 * @since 0.1.0
	 * @param chunk - Raw bytes received from the process stdout stream.
	 * @returns The decoded values terminated by newline bytes in this chunk.
	 */
	Push(chunk: Uint8Array): ReadonlyArray<unknown> {
		const bytes = concat_bytes(this.#pending, chunk);
		const values: Array<unknown> = [];
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
	 * Decodes the final unterminated JSON value when the stream closes.
	 *
	 * @since 0.1.0
	 * @returns The final value, or an empty list when no bytes remain.
	 */
	Finish(): ReadonlyArray<unknown> {
		if (this.#pending.length === 0) {
			return [];
		}

		const values = [decode_line(this.#pending)];

		this.#pending = new Uint8Array();

		return values;
	}
}
