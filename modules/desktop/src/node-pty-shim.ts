import { createRequire } from "node:module";

/** Resolves through the launcher-owned `NODE_PATH`, without ambient configuration. */
const require = createRequire(import.meta.url);
const node_pty = require("node-pty") as {
	readonly spawn: (file: string, args: ReadonlyArray<string>, options: object) => unknown;
};

export const spawn = node_pty.spawn;
