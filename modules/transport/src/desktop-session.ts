import type { ElectronRendererMessagePortShape } from "./electron-message-port";

/** Fixed renderer event name shared by the preload and frontend boundaries. */
export const DesktopSessionConnectionType = "artisan.desktop.connection";

/** A single generation of the narrow desktop preload connection contract. */
export interface DesktopSessionConnection {
	readonly control_port: ElectronRendererMessagePortShape;
	readonly generation: number;
	readonly stream_port: ElectronRendererMessagePortShape;
}

/** The sole capability exposed by Electron preload code to the renderer. */
export interface DesktopSessionBridge {
	readonly requestConnection: () => void | Promise<void>;
}
