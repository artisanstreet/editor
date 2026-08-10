import { Buffer } from "node:buffer";

import { Data, Option, Schema } from "effect";

/** A bounded, byte-faithful line decoded from Claude's stream-json stdout. */
export interface ClaudeJsonlFrame {
	readonly payload: unknown;
	readonly raw_frame_base64: string;
}

export class ClaudeJsonlMalformedLineError extends Data.TaggedError(
	"ClaudeJsonlMalformedLineError",
)<{
	readonly raw_frame_base64: string;
}> {}

export class ClaudeJsonlOversizedLineError extends Data.TaggedError(
	"ClaudeJsonlOversizedLineError",
)<{
	readonly max_frame_bytes: number;
	readonly prefix_base64: string;
	readonly size_bytes: number;
}> {}

export type ClaudeJsonlDecode =
	| ClaudeJsonlFrame
	| ClaudeJsonlMalformedLineError
	| ClaudeJsonlOversizedLineError;

/**
 * Frames arbitrary stdout chunks without decoding them as text first, so raw
 * provenance remains the exact provider bytes and a malformed UTF-8/JSON line
 * cannot poison the following frame.
 */
export class ClaudeJsonlFramer {
	readonly max_frame_bytes: number;
	private buffered = Buffer.alloc(0);
	private discarding_oversized_line = false;

	constructor(options: { readonly max_frame_bytes: number }) {
		this.max_frame_bytes = options.max_frame_bytes;
	}

	PushRecovering(chunk: Uint8Array): ReadonlyArray<ClaudeJsonlDecode> {
		this.buffered = Buffer.concat([this.buffered, chunk]);
		const decoded: Array<ClaudeJsonlDecode> = [];
		while (true) {
			const newline = this.buffered.indexOf(0x0a);
			if (this.discarding_oversized_line) {
				if (newline < 0) {
					this.buffered = Buffer.alloc(0);
					return decoded;
				}
				this.buffered = this.buffered.subarray(newline + 1);
				this.discarding_oversized_line = false;
				continue;
			}
			if (newline < 0) {
				if (this.buffered.byteLength > this.max_frame_bytes) {
					decoded.push(
						new ClaudeJsonlOversizedLineError({
							max_frame_bytes: this.max_frame_bytes,
							prefix_base64: this.buffered
								.subarray(0, this.max_frame_bytes)
								.toString("base64"),
							size_bytes: this.buffered.byteLength,
						}),
					);
					this.buffered = Buffer.alloc(0);
					this.discarding_oversized_line = true;
				}
				return decoded;
			}
			const line = this.buffered.subarray(0, newline);
			this.buffered = this.buffered.subarray(newline + 1);
			if (line.byteLength === 0) continue;
			if (line.byteLength > this.max_frame_bytes) {
				decoded.push(
					new ClaudeJsonlOversizedLineError({
						max_frame_bytes: this.max_frame_bytes,
						prefix_base64: line.subarray(0, this.max_frame_bytes).toString("base64"),
						size_bytes: line.byteLength,
					}),
				);
				continue;
			}
			const raw_frame_base64 = line.toString("base64");
			const parsed = Schema.decodeUnknownOption(Schema.fromJsonString(Schema.Unknown))(
				line.toString("utf8"),
			);
			decoded.push(
				Option.isSome(parsed)
					? { payload: parsed.value, raw_frame_base64 }
					: new ClaudeJsonlMalformedLineError({ raw_frame_base64 }),
			);
		}
	}

	FinishRecovering(): ReadonlyArray<ClaudeJsonlDecode> {
		if (this.discarding_oversized_line) {
			this.buffered = Buffer.alloc(0);
			this.discarding_oversized_line = false;
			return [];
		}
		if (this.buffered.byteLength === 0) return [];
		const trailing = this.buffered;
		this.buffered = Buffer.alloc(0);
		return this.PushRecovering(Buffer.concat([trailing, Buffer.from("\n")]));
	}
}
