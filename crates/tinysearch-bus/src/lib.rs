//! Transport-free `TinySearch` wire contract.
//!
//! Hosts use the member names and payloads here to discover and call search tools.
//! Module configuration is passed privately through `TinyBus` initialization.
pub mod errors;
pub mod labels;
pub mod names;
pub mod render;
pub mod search;
pub mod version;
pub use names::{INTERFACE, METHODS, OBJECT_PATH, role_for_tool, role_tool_name, tools};
pub use search::*;
pub use version::{CONTRACT_VERSION, is_compatible};
