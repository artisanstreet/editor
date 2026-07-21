/** Minimal BrowserWindow capability for the native taskbar/Dock activity signal. */
export interface DesktopWindowProgress {
	readonly setProgressBar: (
		progress: number,
		options?: { readonly mode: "indeterminate" },
	) => void;
}

/**
 * Keeps the native activity indicator idempotent and best-effort. Electron maps this
 * to the platform taskbar/Dock where it is available; no custom tray or overlay UI is
 * introduced for Windows.
 */
export const make_desktop_window_activity = (window: DesktopWindowProgress) => {
	let working = false;

	const SetWorking = (next_working: boolean) => {
		if (working === next_working) return false;
		working = next_working;
		try {
			if (next_working) {
				window.setProgressBar(2, { mode: "indeterminate" });
			} else {
				window.setProgressBar(-1);
			}
		} catch {
			/** Native progress is optional platform polish and must never break the shell. */
		}
		return true;
	};

	return { RestoreIdle: () => SetWorking(false), SetWorking };
};
