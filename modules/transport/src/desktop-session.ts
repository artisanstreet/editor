import type { ElectronRendererMessagePortShape } from "./electron-message-port";
import type { ProjectRef } from "@artisan/protocol";

/** Fixed renderer event name shared by the preload and frontend boundaries. */
export const DesktopSessionConnectionType = "artisan.desktop.connection";

/** A single generation of the narrow desktop preload connection contract. */
export interface DesktopSessionConnection {
	readonly control_port: ElectronRendererMessagePortShape;
	readonly generation: number;
	readonly stream_port: ElectronRendererMessagePortShape;
}

/** Renderer-safe OS identity facts; this is data, never a host inspection capability. */
export interface DesktopIdentity {
	readonly avatar_data_url?: string;
	readonly avatar_seed: string;
	readonly display_name: string;
	readonly machine_name: string;
}

/** The complete narrow capability exposed by Electron preload code to the renderer. */
export interface DesktopSessionBridge {
	readonly forgeWebSocketEndpoint?: string;
	readonly identity: () => Promise<DesktopIdentity>;
	/** Opens the shell-owned directory picker; undefined means the user cancelled. */
	readonly selectProjectDirectory: () => Promise<ProjectRef | undefined>;
	/** Best-effort native taskbar/Dock activity state for the focused desktop shell. */
	readonly setWorking: (working: boolean) => void | Promise<void>;
}
