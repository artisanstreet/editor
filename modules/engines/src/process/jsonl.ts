import { Buffer } from "node:buffer";

import { Data, Schema } from "effect";

/** Represents one malformed UTF-8 or JSON line from an engine stdio transport. @since 0.4.0 */
export class EngineJsonlMalformedLineError extends Data.TaggedError(
	"EngineJsonlMalformedLineError",
)<{
	readonly line_base64: string;
	readonly message: string;
	readonly raw_frame_base64: string;
}> {}

/** Represents one JSONL line discarded after exceeding its configured byte bound. @since 0.4.0 */
export class EngineJsonlOversizedLineError extends Data.TaggedError(
	"EngineJsonlOversizedLineError",
)<{
	readonly max_frame_bytes: number;
	readonly prefix_base64: string;
	readonly size_bytes: number;
}> {}

/** Preserves a decoded JSONL payload together with byte-exact frame provenance. @since 0.4.0 */
export interface EngineJsonlFrame {
	readonly line_base64: string;
	readonly payload: unknown;
	readonly raw_frame_base64: string;
}

/** Represents a decoded JSONL frame or recoverable frame diagnostic. @since 0.4.0 */
export type EngineJsonlDecode =
	| EngineJsonlFrame
	| EngineJsonlMalformedLineError
	| EngineJsonlOversizedLineError;

const oversized_prefix_bytes = 256;

function decode_line(raw_frame: Uint8Array): EngineJsonlDecode {
	const normalized = raw_frame.at(-1) === 13 ? raw_frame.subarray(0, -1) : raw_frame;
	const line_base64 = Buffer.from(normalized).toString("base64");
	const raw_frame_base64 = Buffer.from(raw_frame).toString("base64");
	try {
		const text = new TextDecoder("utf-8", { fatal: true }).decode(normalized);
		return {
			line_base64,
			payload: Schema.decodeUnknownSync(Schema.UnknownFromJsonString)(text),
			raw_frame_base64,
		};
	} catch (cause) {
		return new EngineJsonlMalformedLineError({
			line_base64,
			message: cause instanceof Error ? cause.message : "Unknown JSONL decoding error",
			raw_frame_base64,
		});
	}
}

/** Incrementally frames bounded UTF-8 JSONL and resynchronizes after malformed or oversized lines. @since 0.4.0 */
export class EngineJsonlFramer {
	readonly #max_frame_bytes: number;
	#oversized_prefix = new Uint8Array();
	#oversized_reported = false;
	#oversized_size = 0;
	#pending: Uint8Array;
	#pending_size = 0;
	constructor(options: { readonly max_frame_bytes?: number } = {}) {
		const max_frame_bytes = options.max_frame_bytes ?? Number.MAX_SAFE_INTEGER;
		if (!Number.isSafeInteger(max_frame_bytes) || max_frame_bytes <= 0)
			throw new RangeError("max_frame_bytes must be a positive safe integer");
		this.#max_frame_bytes = max_frame_bytes;
		this.#pending = new Uint8Array(Math.min(1_024, max_frame_bytes));
	}
	#ensure_pending_capacity(required_size: number) {
		if (required_size <= this.#pending.length) return;

		const next_size = Math.min(
			this.#max_frame_bytes,
			Math.max(required_size, this.#pending.length * 2),
		);
		const next = new Uint8Array(next_size);

		next.set(this.#pending.subarray(0, this.#pending_size));
		this.#pending = next;
	}
	#append_segment(segment: Uint8Array) {
		if (this.#oversized_size > 0) {
			this.#oversized_size += segment.length;
			return;
		}
		const size_bytes = this.#pending_size + segment.length;
		if (size_bytes <= this.#max_frame_bytes) {
			this.#ensure_pending_capacity(size_bytes);
			this.#pending.set(segment, this.#pending_size);
			this.#pending_size = size_bytes;
			return;
		}
		const prefix = new Uint8Array(Math.min(oversized_prefix_bytes, this.#max_frame_bytes));
		const pending_length = Math.min(this.#pending_size, prefix.length);

		prefix.set(this.#pending.subarray(0, pending_length));
		if (pending_length < prefix.length)
			prefix.set(segment.subarray(0, prefix.length - pending_length), pending_length);
		this.#oversized_prefix = prefix;
		this.#oversized_size = size_bytes;
		this.#pending_size = 0;
	}
	#finish_line(): EngineJsonlDecode | undefined {
		if (this.#oversized_size > 0) {
			const value = this.#oversized_reported
				? undefined
				: new EngineJsonlOversizedLineError({
						max_frame_bytes: this.#max_frame_bytes,
						prefix_base64: Buffer.from(this.#oversized_prefix).toString("base64"),
						size_bytes: this.#oversized_size,
					});
			this.#oversized_prefix = new Uint8Array();
			this.#oversized_reported = false;
			this.#oversized_size = 0;
			return value;
		}
		const value = decode_line(this.#pending.subarray(0, this.#pending_size));
		this.#pending_size = 0;
		return value;
	}
	/** Accepts raw bytes and emits complete frames while retaining recoverable diagnostics. @since 0.4.0 */
	PushRecovering(chunk: Uint8Array): ReadonlyArray<EngineJsonlDecode> {
		const values: Array<EngineJsonlDecode> = [];
		let start = 0;
		for (let index = 0; index < chunk.length; index += 1) {
			if (chunk[index] !== 10) continue;
			this.#append_segment(chunk.subarray(start, index));
			const value = this.#finish_line();
			if (value !== undefined) values.push(value);
			start = index + 1;
		}
		this.#append_segment(chunk.subarray(start));
		if (this.#oversized_size > 0 && !this.#oversized_reported) {
			this.#oversized_reported = true;
			values.push(
				new EngineJsonlOversizedLineError({
					max_frame_bytes: this.#max_frame_bytes,
					prefix_base64: Buffer.from(this.#oversized_prefix).toString("base64"),
					size_bytes: this.#oversized_size,
				}),
			);
		}
		return values;
	}
	/** Decodes final unterminated data while retaining a recoverable diagnostic. @since 0.4.0 */
	FinishRecovering(): ReadonlyArray<EngineJsonlDecode> {
		if (this.#pending_size === 0 && this.#oversized_size === 0) return [];
		const value = this.#finish_line();
		return value === undefined ? [] : [value];
	}
	/** Decodes complete frames and fails on the first recoverable diagnostic. @since 0.4.0 */
	Push(chunk: Uint8Array): ReadonlyArray<unknown> {
		const values = this.PushRecovering(chunk);
		const error = values.find(
			(value) =>
				value instanceof EngineJsonlMalformedLineError ||
				value instanceof EngineJsonlOversizedLineError,
		);
		if (error) throw error;
		return values.flatMap((value) =>
			value instanceof EngineJsonlMalformedLineError ||
			value instanceof EngineJsonlOversizedLineError
				? []
				: [value.payload],
		);
	}
	/** Decodes final data and fails on a recoverable diagnostic. @since 0.4.0 */
	Finish(): ReadonlyArray<unknown> {
		const values = this.FinishRecovering();
		const error = values.find(
			(value) =>
				value instanceof EngineJsonlMalformedLineError ||
				value instanceof EngineJsonlOversizedLineError,
		);
		if (error) throw error;
		return values.flatMap((value) =>
			value instanceof EngineJsonlMalformedLineError ||
			value instanceof EngineJsonlOversizedLineError
				? []
				: [value.payload],
		);
	}
}
