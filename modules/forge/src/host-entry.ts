import { NodeRuntime } from "@effect/platform-node-shared";
import { MakeSnowflakeIdLive } from "@artisan/protocol";
import { Effect, Layer } from "effect";

import { StartForgeFromEnvironment } from "./entry";

NodeRuntime.runMain(
	StartForgeFromEnvironment.pipe(Effect.provide(MakeSnowflakeIdLive(3).pipe(Layer.orDie))),
);
