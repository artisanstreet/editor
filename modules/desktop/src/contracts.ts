/** Structural Electron surface kept injectable for deterministic shell tests. */
export interface DesktopPortShape {
	readonly close: () => void;
}

/** Renderer-safe OS identity projection, never a capability to inspect the host. */
export interface DesktopIdentity {
	readonly avatar_data_url?: string;
	readonly avatar_seed: string;
	readonly display_name: string;
	readonly machine_name: string;
}

export interface DesktopSmokeConnection {
	readonly control_port: DesktopPortShape;
	readonly generation: number;
	readonly stream_port: DesktopPortShape;
}

/** The small BrowserWindow surface used exclusively by the packaged release smoke. */
export interface DesktopSmokeRenderer {
	/** Makes the native window the focused target for Electron input injection. */
	readonly focus: () => void;
	readonly isFocused: () => boolean;
	readonly moveTop: () => void;
	readonly restore: () => void;
	readonly setBounds: (bounds: { readonly height: number; readonly width: number }) => void;
	readonly show: () => void;
	readonly webContents: {
		readonly executeJavaScript: (code: string, user_gesture?: boolean) => Promise<unknown>;
		readonly focus: () => void;
		readonly getZoomFactor: () => number;
		readonly sendInputEvent: (
			input:
				| {
						readonly keyCode: string;
						readonly type: "char" | "keyDown" | "keyUp" | "rawKeyDown";
				  }
				| {
						readonly button: "left";
						readonly clickCount?: number;
						readonly type: "mouseDown" | "mouseUp";
						readonly x: number;
						readonly y: number;
				  }
				| {
						readonly type: "mouseMove";
						readonly x: number;
						readonly y: number;
				  },
		) => void;
		readonly setZoomFactor: (factor: number) => void;
	};
}

export interface DesktopMessageChannelShape {
	readonly port1: DesktopPortShape;
	readonly port2: DesktopPortShape;
}

export interface DesktopUtilityProcessShape {
	readonly kill: () => boolean;
	readonly on: (...args: ReadonlyArray<unknown>) => unknown;
	/** Electron exposes this for audit evidence; deterministic tests may omit it. */
	readonly pid?: number;
	readonly postMessage: (message: unknown, transfer?: Array<DesktopPortShape>) => void;
}

export interface DesktopWebContentsShape {
	readonly id: number;
	readonly isDestroyed: () => boolean;
	readonly postMessage: (
		channel: string,
		message: unknown,
		transfer: Array<DesktopPortShape>,
	) => void;
}

export interface DesktopIpcEventShape {
	readonly sender: DesktopWebContentsShape;
}

export interface DesktopPaths {
	readonly database_path: string;
	readonly frontend_index_path: string;
	readonly frontend_root: string;
	readonly migrations_path: string;
	readonly preload_path: string;
	readonly utility_path: string;
}
