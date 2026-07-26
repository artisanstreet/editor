/**
 * The Electron shell is fully bundled by Vite. Returning false tells
 * electron-builder that production dependencies are already handled and avoids
 * traversing the repository's pnpm workspace for modules that are not shipped.
 */
export default async () => false;
