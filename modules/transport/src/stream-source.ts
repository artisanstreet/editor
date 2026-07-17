import { Context, Data, Effect, Layer, Option, Stream } from "effect";

import { RichLinkAssetStore, TerminalSessionService } from "@artisan/backend";

import type { MessagePortTerminalStreamContext, TerminalOutputGapDetail } from "./wire";

/** Identifies a binary stream source failure safe to expose across a port. */
export type BinaryStreamSourceErrorCode = "gap" | "not_found" | "source_error" | "unsupported";

/** Reports why a terminal or asset byte stream could not be opened or read. */
export class BinaryStreamSourceError extends Data.TaggedError("BinaryStreamSourceError")<{
	readonly cause: unknown;
	readonly code: BinaryStreamSourceErrorCode;
	readonly stream_id: string;
	readonly terminal_output_gap?: TerminalOutputGapDetail;
}> {}

/** Supplies one schema-decoded terminal or asset stream request. */
export interface BinaryStreamSourceOpenInput {
	readonly stream_id: string;
	readonly terminal_context?: MessagePortTerminalStreamContext;
}

/** Opens bounded terminal or asset byte streams by provider-neutral stream id. */
export interface BinaryStreamSourceShape {
	readonly Open: (
		input: BinaryStreamSourceOpenInput,
	) => Effect.Effect<Stream.Stream<Uint8Array, BinaryStreamSourceError>, BinaryStreamSourceError>;
}

/** Opens bounded terminal or asset byte streams by provider-neutral stream id. */
export class BinaryStreamSource extends Context.Service<
	BinaryStreamSource,
	BinaryStreamSourceShape
>()("Artisan/BinaryStreamSource") {}

function source_error(
	stream_id: string,
	code: BinaryStreamSourceErrorCode,
	cause: unknown,
	terminal_output_gap?: TerminalOutputGapDetail,
) {
	return new BinaryStreamSourceError({
		cause,
		code,
		stream_id,
		...(terminal_output_gap === undefined ? {} : { terminal_output_gap }),
	});
}

/** Adapts live backend terminal output and retained rich-link assets to byte streams. */
export const BackendBinaryStreamSourceLive = Layer.effect(
	BinaryStreamSource,
	Effect.gen(function* () {
		const assets = yield* RichLinkAssetStore;
		const terminals = yield* TerminalSessionService;

		const open = (input: BinaryStreamSourceOpenInput) => {
			const stream_id = input.stream_id;

			if (stream_id.startsWith("terminal:")) {
				const terminal_id = stream_id.slice("terminal:".length);
				const terminal_context = input.terminal_context;

				if (
					terminal_context === undefined ||
					terminal_context.terminal_id !== terminal_id
				) {
					return Effect.fail(
						source_error(
							stream_id,
							"not_found",
							new Error("terminal stream ownership context is invalid"),
						),
					);
				}

				return terminals
					.Output(terminal_id, terminal_context.thread_id, terminal_context.workspace_id)
					.pipe(
						Effect.map((output) =>
							output.pipe(
								Stream.mapEffect((event) =>
									event._tag === "chunk"
										? Effect.succeed(Uint8Array.from(event.data))
										: Effect.fail(source_error(stream_id, "gap", event, event)),
								),
								Stream.mapError((cause) =>
									cause instanceof BinaryStreamSourceError
										? cause
										: source_error(stream_id, "source_error", cause),
								),
							),
						),
						Effect.mapError((cause) => source_error(stream_id, "not_found", cause)),
					);
			}

			if (input.terminal_context !== undefined) {
				return Effect.fail(
					source_error(
						stream_id,
						"unsupported",
						new Error("terminal stream context is invalid for this stream namespace"),
					),
				);
			}

			if (stream_id.startsWith("asset:")) {
				const asset_id = stream_id.slice("asset:".length);

				return assets.Get(asset_id).pipe(
					Effect.flatMap((asset) =>
						Option.match(asset, {
							onNone: () =>
								Effect.fail(
									source_error(
										stream_id,
										"not_found",
										new Error("rich-link asset was not retained"),
									),
								),
							onSome: (stored) => Effect.succeed(Stream.succeed(stored.body)),
						}),
					),
				);
			}

			return Effect.fail(
				source_error(
					stream_id,
					"unsupported",
					new Error("stream id must use the terminal: or asset: namespace"),
				),
			);
		};

		return { Open: open };
	}),
);
