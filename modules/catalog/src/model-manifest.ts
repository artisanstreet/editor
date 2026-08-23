import { Schema } from "effect";

import { ModelManifest } from "./schema";
import { anthropic_models } from "./manifest/anthropic";
import { cursor_anthropic_models } from "./manifest/cursor-anthropic";
import { cursor_core_models } from "./manifest/cursor-core";
import { cursor_openai_models } from "./manifest/cursor-openai";
import { cursor_other_models } from "./manifest/cursor-other";
import { harnesses, opencode2_big_pickle_compaction_model_id } from "./manifest/harnesses";
import { openai_models } from "./manifest/openai";
import { thinking_level_labels } from "./manifest/options";
import { providers } from "./manifest/providers";
import { xai_models } from "./manifest/xai";

export { thinking_level_labels };
export { opencode2_big_pickle_compaction_model_id };

export const model_manifest = Schema.decodeUnknownSync(ModelManifest)({
	revision: "2026-08-21.2",
	providers,
	harnesses,
	models: [
		...openai_models,
		...anthropic_models,
		...xai_models,
		...cursor_core_models,
		...cursor_openai_models,
		...cursor_anthropic_models,
		...cursor_other_models,
	],
});
