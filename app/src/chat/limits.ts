// Limits the views use, mirrored from `crates/atrium-protocol/src/limits.rs` (the generated
// bindings carry types, not constants). limits.test.ts reads the Rust file and fails when one
// of these drifts from it.

/** Rendered transcript a thread view keeps loaded (text and tool output), contract §11. */
export const MAX_LOADED_TRANSCRIPT_BYTES = 8 * 1024 * 1024;
/** `data:` raster images rendered inline in agent content (contract §12). */
export const MAX_INLINE_IMAGE_BYTES = 1024 * 1024;
/** A tool call's output, all deltas together (contract §6.1). */
export const MAX_TOOL_OUTPUT_BYTES = 1024 * 1024;
/** Pending or approved requests per incarnation (contract §5). */
export const MAX_PENDING_REQUESTS = 64;
