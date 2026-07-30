import { createHash } from "node:crypto";

import type { ModelBehaviourValue } from "@artisan/protocol";

/** Returns the content-free canonical identity shared by every provider mapping. */
export function hash_model_behaviour_value(value: ModelBehaviourValue) {
	return createHash("sha256").update(JSON.stringify(value)).digest("hex");
}
