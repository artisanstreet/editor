import { Schema } from "effect";

/** The two packaged treatments the desktop shell can apply at runtime. */
export const DesktopAppIconPreference = Schema.Literals([
	"plastic-jaw-shading",
	"foreground-gradient-symbol",
]);
export type DesktopAppIconPreference = typeof DesktopAppIconPreference.Type;

export const default_desktop_app_icon: DesktopAppIconPreference = "foreground-gradient-symbol";

/** Same-origin control surface exposed only by the bundled Electron renderer host. */
export const desktop_app_icon_control_path = "/.artisan/desktop/app-icon";

export const DesktopAppIconSelection = Schema.Struct({
	icon: DesktopAppIconPreference,
});
export type DesktopAppIconSelection = typeof DesktopAppIconSelection.Type;
