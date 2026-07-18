import type { DesktopSessionBridge } from "@artisan/transport/client";

declare global {
	namespace App {}

	interface Window {
		/** Narrow, preload-owned desktop bridge. Ports arrive separately as a window message. */
		readonly artisanDesktop?: DesktopSessionBridge;
	}
}

export {};
