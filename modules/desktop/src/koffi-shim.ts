import { createRequire } from "node:module";

const require = createRequire(import.meta.url);

/** Loads the staged Windows Koffi binding without consulting ambient Node resolution. */
const koffi = require("./native-runtime/@koromix/koffi-win32-x64") as unknown;

export default koffi;
