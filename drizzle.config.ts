import { defineConfig } from "drizzle-kit";

export default defineConfig({
	dialect: "sqlite",
	out: "./modules/backend/drizzle",
	schema: "./modules/backend/src/persistence/schema.ts",
});
