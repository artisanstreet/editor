/**
 * Public, non-secret navigation marker that forces a fresh renderer document
 * after the background Forge handoff is ready. Pairing capabilities remain in
 * the URL fragment and never enter the custom-protocol request.
 */
export const desktop_handoff_navigation_parameter = "artisan-handoff";
