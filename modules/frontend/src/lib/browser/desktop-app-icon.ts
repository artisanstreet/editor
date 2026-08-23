import {
	desktop_app_icon_control_path,
	DesktopAppIconSelection,
	type DesktopAppIconPreference,
} from "@artisan/protocol";
import { Data, Effect, Schema } from "effect";

export class DesktopAppIconRequestError extends Data.TaggedError("DesktopAppIconRequestError")<{
	readonly cause?: unknown;
	readonly message: string;
}> {}

export const DesktopAppIconsAvailable = (protocol: string): boolean => protocol === "artisan:";

const BrowserLocation = () =>
	(
		globalThis as {
			readonly location?: { readonly origin: string; readonly protocol: string };
		}
	).location;

const Endpoint = () => {
	const location = BrowserLocation();
	if (location === undefined || !DesktopAppIconsAvailable(location.protocol)) {
		throw new DesktopAppIconRequestError({
			message: "App icon selection is available only in the Artisan desktop app.",
		});
	}
	return new URL(desktop_app_icon_control_path, location.origin).href;
};

const Request = (init?: RequestInit) =>
	Effect.tryPromise({
		catch: (cause) =>
			cause instanceof DesktopAppIconRequestError
				? cause
				: new DesktopAppIconRequestError({
						cause,
						message: "The desktop app icon could not be changed.",
					}),
		try: async () => {
			const response = await fetch(Endpoint(), init);
			if (!response.ok) {
				throw new DesktopAppIconRequestError({
					message: `The desktop app icon request failed (${response.status}).`,
				});
			}
			return response;
		},
	});

export const LoadDesktopAppIcon = Effect.gen(function* () {
	const response = yield* Request();
	const payload = yield* Effect.tryPromise({
		catch: (cause) =>
			new DesktopAppIconRequestError({
				cause,
				message: "The desktop app returned an invalid icon preference.",
			}),
		try: () => response.json() as Promise<unknown>,
	});
	const selection = yield* Schema.decodeUnknownEffect(DesktopAppIconSelection)(payload).pipe(
		Effect.mapError(
			(cause) =>
				new DesktopAppIconRequestError({
					cause,
					message: "The desktop app returned an invalid icon preference.",
				}),
		),
	);
	return selection.icon;
});

export const SelectDesktopAppIcon = (icon: DesktopAppIconPreference) =>
	Request({
		body: JSON.stringify({ icon }),
		headers: { "content-type": "application/json" },
		method: "PUT",
	}).pipe(Effect.asVoid);
