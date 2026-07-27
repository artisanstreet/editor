import { Option, Schema } from "effect";

export const ForgeStartLaunchUrl = "artisan://forge/start";

export const ForgeStartLaunchRequest = Schema.Struct({
	action: Schema.Literal("start"),
	target: Schema.Literal("forge"),
});

export type ForgeStartLaunchRequest = typeof ForgeStartLaunchRequest.Type;

const decode_request = Schema.decodeUnknownOption(ForgeStartLaunchRequest);

/** Accepts only the fixed, argument-free Forge start capability. */
export const DecodeForgeStartLaunchRequest = (
	input: unknown,
): Option.Option<ForgeStartLaunchRequest> => {
	if (typeof input !== "string") return Option.none();

	try {
		const url = new URL(input);
		if (
			url.protocol !== "artisan:" ||
			url.hostname !== "forge" ||
			url.pathname !== "/start" ||
			url.username !== "" ||
			url.password !== "" ||
			url.port !== "" ||
			url.search !== "" ||
			url.hash !== ""
		) {
			return Option.none();
		}

		return decode_request({ action: "start", target: "forge" });
	} catch {
		return Option.none();
	}
};

export const FindForgeStartLaunchRequest = (
	arguments_: ReadonlyArray<string>,
): Option.Option<ForgeStartLaunchRequest> =>
	Option.firstSomeOf(arguments_.map(DecodeForgeStartLaunchRequest));
