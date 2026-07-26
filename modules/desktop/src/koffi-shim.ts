import { createRequire } from "node:module";

const require = createRequire(import.meta.url);

/**
 * Native runtime discovery is owned by the launcher's `NODE_PATH`; this shim
 * never reads ambient configuration during module evaluation.
 */
const koffi = require("koffi") as unknown;

export default koffi;
