import { build } from "rolldown";

import { CreateForgeRolldownConfig, type ForgeBuildMode } from "../../forge.rolldown.config.ts";

const requested_mode = process.argv[2];

if (requested_mode !== "production" && requested_mode !== "validation") {
	throw new Error("Forge build mode must be production or validation");
}

const mode: ForgeBuildMode = requested_mode;

await build(CreateForgeRolldownConfig({ mode }));
