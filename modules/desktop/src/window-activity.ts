import { Effect, Ref } from "effect";

/** Minimal BrowserWindow capability for the native taskbar/Dock activity signal. */
export interface DesktopWindowProgress {
	readonly setProgressBar: (
		progress: number,
		options?: { readonly mode: "indeterminate" },
	) => void;
}

/** Acquires Effect-owned, idempotent native taskbar/Dock activity state. */
export const make_desktop_window_activity = (window: DesktopWindowProgress) =>
	Effect.gen(function* () {
		const working = yield* Ref.make(false);
		const SetWorking = (next_working: boolean) =>
			Ref.modify(working, (current) => [current !== next_working, next_working]).pipe(
				Effect.tap((changed) =>
					!changed
						? Effect.void
						: Effect.try({
								try: () =>
									next_working
										? window.setProgressBar(2, { mode: "indeterminate" })
										: window.setProgressBar(-1),
								catch: () => undefined,
							}).pipe(Effect.ignore),
				),
			);

		return { RestoreIdle: SetWorking(false), SetWorking };
	});
