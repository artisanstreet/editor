# @artisanstreet/runner

An Effect-native development process runner with a bounded interactive terminal dashboard.

```ts
import { NodeRuntime } from "@effect/platform-node-shared";
import { ChildProcess } from "effect/unstable/process";

import { Runner } from "@artisanstreet/runner";

Runner.make([
	{
		name: "API",
		command: ChildProcess.make`pnpm run dev:api`,
		readiness: Runner.Readiness.output(/listening/u),
	},
	{
		name: "Web",
		command: ChildProcess.make`pnpm run dev:web`,
		readiness: Runner.Readiness.http("http://127.0.0.1:5173"),
	},
]).pipe(NodeRuntime.runMain);
```

`Runner.make` supplies its complete Node implementation and returns a closed
`Effect<never, Runner.Error>` whose scope owns every child process and terminal resource.
Managed development commands are expected to remain alive; any natural exit, including exit
code zero, is treated as an unexpected runner failure after stdout and stderr are drained.
