import type { ProviderDefinition } from "../schema";

export const providers = [
	{ id: "openai", label: "OpenAI" },
	{ id: "anthropic", label: "Anthropic" },
	{ id: "xai", label: "xAI" },
	{ id: "opencode", label: "OpenCode" },
	{ id: "deepseek", label: "DeepSeek" },
	{ id: "meta", label: "Meta" },
	{ id: "minimax", label: "MiniMax" },
	{ id: "nvidia", label: "NVIDIA" },
	{ id: "qwen", label: "Qwen" },
	{ id: "tencent", label: "Tencent" },
	{ id: "xiaomi", label: "Xiaomi" },
	{ id: "unknown", label: "Unknown" },
	{ id: "cursor", label: "Cursor" },
	{ id: "google", label: "Google" },
	{ id: "moonshot", label: "Moonshot" },
	{ id: "zai", label: "Z.ai" },
] satisfies ReadonlyArray<ProviderDefinition>;
