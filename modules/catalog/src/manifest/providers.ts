import type { ProviderDefinition } from "../schema";

export const providers = [
	{ id: "openai", label: "OpenAI" },
	{ id: "anthropic", label: "Anthropic" },
	{ id: "xai", label: "xAI" },
	{ id: "cursor", label: "Cursor" },
	{ id: "google", label: "Google" },
	{ id: "moonshot", label: "Moonshot" },
	{ id: "zai", label: "Z.ai" },
] satisfies ReadonlyArray<ProviderDefinition>;
