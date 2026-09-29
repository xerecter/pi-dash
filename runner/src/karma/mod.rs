//! Karma agent integration.
//!
//! `karma` (the Karma CLI) wraps Claude Code: it accepts the exact same
//! headless invocation
//! (`karma --print --input-format stream-json --output-format stream-json
//! --include-partial-messages …`) and emits the identical stream-json event
//! schema. Rather than duplicate the bridge, the Karma agent re-uses the
//! Claude Code implementation verbatim — only the configured executable
//! differs (`karma` instead of `claude`).
//!
//! See [`crate::claude_code`] for the protocol details, subprocess lifecycle,
//! and event translation. If Karma's CLI ever diverges from Claude Code's,
//! fork the relevant piece here rather than editing the shared bridge.

pub use crate::claude_code::{bridge, process, schema};
