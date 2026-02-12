//! Agentic tools for LLM-driven codebase exploration.
//!
//! These tools allow the LLM to explore the repository when
//! running in agentic mode (`--agent`). Each tool implements
//! rig-core's `Tool` trait for native tool calling support.
//!
//! In addition to the built-in tools, users can define custom
//! command-line tools in their agent profile's YAML frontmatter.
//! See [`custom_command::CustomCommandTool`] for details.

pub mod custom_command;
pub mod list_directory;
pub mod read_file;
pub mod search_text;

// Re-export the rig Tool wrapper types
pub use custom_command::CustomCommandTool;
pub use list_directory::ListDirectoryTool;
pub use read_file::ReadFileTool;
pub use search_text::SearchTextTool;
