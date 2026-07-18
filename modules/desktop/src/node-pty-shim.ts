import { createRequire } from "node:module";

/** Loads the explicitly staged PTY package beside the bundled utility entry. */
const require = createRequire(import.meta.url);
const node_pty = require("./native-runtime/node-pty") as {
	readonly spawn: (file: string, args: ReadonlyArray<string>, options: object) => unknown;
};

export const spawn = node_pty.spawn;
