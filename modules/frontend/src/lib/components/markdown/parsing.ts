/**
 * The conversation markdown dialect. Assistant output is untrusted, so raw
 * HTML stays inert text instead of becoming live elements. Markdown-generated
 * elements and markdown-it's link-protocol vetting remain in effect, and
 * unterminated syntax is auto-closed so partial streamed messages render
 * cleanly at every frame.
 */
export const conversation_parse_options = { html: false } as const;
