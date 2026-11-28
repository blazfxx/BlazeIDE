//! The agent's way onto the web, for the IDE chat, the CLI, phone chats and
//! BlBots:
//!   web_search    search the web
//!   web_fetch     read a page as text (fast, no browser)
//!   web_browser   a real browser: open, look, scroll, back, screenshot
//!   web_interact  click and type in that browser
//! Everything a page says reaches the model marked as untrusted content.

pub mod chrome;
pub mod fetch;
pub mod guard;
mod proxy;
pub mod search;
pub mod tool;

pub use guard::Policy;

/// Wrap page content so the model can tell it from instructions. A page
/// cannot close the wrapper early.
pub(crate) fn untrusted(source: &str, content: &str) -> String {
    let safe = content.replace("</web_content", "</ web_content").replace("<web_content", "< web_content");
    let source: String = source.chars().filter(|c| !matches!(c, '"' | '<' | '>' | '\n')).take(300).collect();
    format!("<web_content source=\"{source}\">\n{safe}\n</web_content>")
}

#[cfg(test)]
mod tests;
