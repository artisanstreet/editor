import { build } from "rolldown";

import { CreateForgeRolldownConfig, type ForgeBuildMode } from "../../forge.rolldown.config.ts";
import { BuildForgeSea } from "./build-forge-sea.ts";

const requested_mode = process.argv[2];

if (requested_mode !== "production" && requested_mode !== "validation") {
	throw new Error("Forge build mode must be production or validation");
}

const mode: ForgeBuildMode = requested_mode;

if (mode === "production") await BuildForgeSea();
else await build(CreateForgeRolldownConfig({ mode }));
