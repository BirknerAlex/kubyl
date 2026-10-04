//! Terminal sessions without the UI.
//!
//! - [`exec`]: pod exec sessions (attach, stdin, resize, stdout/stderr).
//! - [`grid`]: the terminal grid on `alacritty_terminal` (screen, scrollback, selection).
//! - [`input`]: key input to terminal byte sequences.
//! - [`shell`]: which shell to start in a container.
//! - [`settings`]: the `"terminal"` settings.json section.
//!
//! `kubyl_terminal` re-exports these modules and adds the terminal view, panel and dialogs.

pub mod exec;
pub mod grid;
pub mod input;
pub mod settings;
pub mod shell;
