import { NodeRuntime } from "@effect/platform-node-shared";

import { WindowsProcessHostProgram } from "./windows-process-host.ts";

/** The legacy loose helper has exactly one Effect runtime boundary. */
NodeRuntime.runMain(WindowsProcessHostProgram);
