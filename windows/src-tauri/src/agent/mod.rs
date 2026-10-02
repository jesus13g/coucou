// The agent loop behind Mochi's chat: a turn runs model calls and local tool
// rounds until the model answers in text, the user stops it, or a step cap is
// hit. Nothing here depends on Tauri, so the whole loop runs in unit tests
// against a mock transport and fake tools.

pub mod cancel;
pub mod events;
pub mod transport;
pub mod tools;
