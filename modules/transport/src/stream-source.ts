import { Context, Data, Effect, Layer, Option, Stream } from "effect";

import { PreviewCoordinator, TerminalSessionService } from "@artisan/backend";

/** Identifies a binary stream source failure safe to expose across a port. */
export type BinaryStreamSourceErrorCode = "not_found" | "source_error" | "unsupported";

/** Reports why a terminal or asset byte stream could not be opened or read. */
export class BinaryStreamSourceError extends Data.TaggedError("BinaryStreamSourceError")<{
	readonly cause: unknown;
	readonly code: BinaryStreamSourceErrorCode;
	readonly stream_id: string;
}> {}

/** Opens bounded terminal or asset byte streams by provider-neutral stream id. */
export interface BinaryStreamSourceShape {
	readonly Open: (
		stream_id: string,
	) => Effect.Effect<Stream.Stream<Uint8Array, BinaryStreamSourceError>, BinaryStreamSourceError>;
}

/** Opens bounded terminal or asset byte streams by provider-neutral stream id. */
export class BinaryStreamSource extends Context.Service<
	BinaryStreamSource,
	BinaryStreamSourceShape
>()("Artisan/BinaryStreamSource") {}

function source_error(stream_id: string, code: BinaryStreamSourceErrorCode, cause: unknown) {
	return new BinaryStreamSourceError({ cause, code, stream_id });
}

/** Adapts live backend terminal output and retained rich-link assets to byte streams. */
export const BackendBinaryStreamSourceLive = Layer.effect(
	BinaryStreamSource,
	Effect.gen(function* () {
		const previews = yield* PreviewCoordinator;
		const terminals = yield* TerminalSessionService;

		const open = (stream_id: string) => {
			if (stream_id.startsWith("terminal:")) {
				const parts = stream_id.slice("terminal:".length).split(":");
				if (parts.length !== 3) {
					return Effect.fail(
						source_error(
							stream_id,
							"unsupported",
							new Error("terminal stream scope is required"),
						),
					);
				}
				const [thread_id, workspace_id, terminal_id] = parts.map((part) =>
					decodeURIComponent(part),
				);
				if (
					thread_id === undefined ||
					workspace_id === undefined ||
					terminal_id === undefined
				) {
					return Effect.fail(
						source_error(
							stream_id,
							"unsupported",
							new Error("terminal stream scope is invalid"),
						),
					);
				}

				return terminals.Output({ terminal_id, thread_id, workspace_id }).pipe(
					Effect.map((output) =>
						output.pipe(
							Stream.mapError((cause) =>
								source_error(stream_id, "source_error", cause),
							),
						),
					),
					Effect.mapError((cause) => source_error(stream_id, "source_error", cause)),
				);
			}

			if (stream_id.startsWith("asset:")) {
				const asset_id = stream_id.slice("asset:".length);
				const valid_asset_id = /^[a-f0-9]{64}$/u.test(asset_id);

				if (!valid_asset_id) {
					return Effect.fail(
						source_error(
							stream_id,
							"unsupported",
							new Error("asset streams require a lowercase SHA-256 asset id"),
						),
					);
				}

				return previews.Asset(asset_id).pipe(
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
					Effect.mapError((cause) => source_error(stream_id, "not_found", cause)),
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
