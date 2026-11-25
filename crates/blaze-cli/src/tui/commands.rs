//! The slash commands: one list drives the `/` menu, /help and dispatch.

pub struct Cmd {
    pub group: &'static str,
    pub name: &'static str,
    pub args: &'static str,
    pub desc: &'static str,
    pub aliases: &'static [&'static str],
}

pub const COMMANDS: &[Cmd] = &[
    Cmd { group: "Connect", name: "rc", args: "[relay|tunnel|lan|off]", desc: "Remote connect: continue on your phone", aliases: &["remote", "mobile"] },
    Cmd { group: "Chat", name: "new", args: "", desc: "Start a new chat", aliases: &["clear"] },
    Cmd { group: "Chat", name: "chats", args: "", desc: "Open a saved chat (shared with Blaze IDE)", aliases: &["resume", "sessions"] },
    Cmd { group: "Chat", name: "model", args: "[name]", desc: "Pick a model (BlazeAPI or your own provider)", aliases: &["models"] },
    Cmd { group: "Chat", name: "mode", args: "ask|plan|agent", desc: "Ask = talk only, Plan = plan first, Agent = do it", aliases: &[] },
    Cmd { group: "Chat", name: "approval", args: "ask|edits|full", desc: "What the agent may do without asking", aliases: &["permissions"] },
    Cmd { group: "Chat", name: "bots", args: "", desc: "Talk to one of your BlBots", aliases: &["bot"] },
    Cmd { group: "Edit", name: "undo", args: "", desc: "Undo the last turn and its file changes", aliases: &["revert"] },
    Cmd { group: "Edit", name: "init", args: "", desc: "Write an AGENTS.md that explains this project", aliases: &[] },
    Cmd { group: "Edit", name: "copy", args: "", desc: "Copy the last answer", aliases: &[] },
    Cmd { group: "Edit", name: "export", args: "", desc: "Save this chat as Markdown in the project", aliases: &[] },
    Cmd { group: "Account", name: "status", args: "", desc: "Account, balance, model and connections", aliases: &["whoami"] },
    Cmd { group: "Account", name: "provider", args: "add|key|models|rm", desc: "Your own provider (called directly, not via BlazeAPI)", aliases: &["providers", "byok"] },
    Cmd { group: "Account", name: "login", args: "", desc: "Sign in with BlazeAPI (opens your browser)", aliases: &[] },
    Cmd { group: "Account", name: "logout", args: "", desc: "Sign out on this computer", aliases: &[] },
    Cmd { group: "App", name: "update", args: "[auto|notify|off]", desc: "Update blaze now, or choose automatic updates", aliases: &["upgrade"] },
    Cmd { group: "App", name: "theme", args: "auto|dark|light", desc: "Colours for a light or dark terminal", aliases: &["colors", "colours"] },
    Cmd { group: "Account", name: "discord", args: "on|off", desc: "Show \"Working on ...\" as your Discord status", aliases: &["presence"] },
    Cmd { group: "Account", name: "setup", args: "", desc: "Run the first-time setup again", aliases: &["onboard"] },
    Cmd { group: "App", name: "help", args: "", desc: "Keys and commands", aliases: &["?"] },
    Cmd { group: "App", name: "exit", args: "", desc: "Quit (the chat is saved)", aliases: &["quit", "q"] },
];

/// Commands matching what is typed after `/` (prefix on name/alias first, then substring).
pub fn matching(typed: &str) -> Vec<&'static Cmd> {
    let t = typed.trim_start_matches('/').split_whitespace().next().unwrap_or("").to_lowercase();
    // Name prefix, then alias prefix, then anywhere in the name or description.
    let mut out: Vec<&Cmd> = COMMANDS.iter().filter(|c| c.name.starts_with(&t)).collect();
    for c in COMMANDS {
        let hit = c.aliases.iter().any(|a| a.starts_with(&t)) || c.name.contains(&t) || c.desc.to_lowercase().contains(&t);
        if hit && !out.iter().any(|o| o.name == c.name) {
            out.push(c);
        }
    }
    out
}

/// The command a typed name refers to.
pub fn find(name: &str) -> Option<&'static Cmd> {
    let n = name.trim_start_matches('/').to_lowercase();
    COMMANDS.iter().find(|c| c.name == n || c.aliases.contains(&n.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups() {
        assert_eq!(find("/remote").unwrap().name, "rc");
        assert_eq!(find("quit").unwrap().name, "exit");
        assert_eq!(matching("/r")[0].name, "rc");
        assert_eq!(matching("/mo")[0].name, "model");
        assert!(matching("/").len() == COMMANDS.len());
        assert!(matching("/phone").iter().any(|c| c.name == "rc"), "description search");
    }
}
