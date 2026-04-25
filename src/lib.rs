pub const APP_NAME: &str = "cxv";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn version_line() -> String {
    format!("{APP_NAME} {APP_VERSION}\n")
}

pub mod cli;
pub mod parser;
pub mod tui;
