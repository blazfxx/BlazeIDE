//! `blaze`: Blaze IDE in the terminal.
//!
//!   blaze                      chat in the current folder (full-screen)
//!   blaze -p "fix the tests"   one turn, print the answer, exit
//!   blaze login | logout | whoami | models | chats

mod bots;
mod config;
mod oneshot;
mod rc;
mod serve;
mod session;
mod tui;

use std::io::IsTerminal;
use std::path::PathBuf;

use blaze_core::agent::Approval;
use clap::{Parser, Subcommand};
use serde_json::json;

use session::Session;

/// Stamped by CI (same number as the Blaze IDE release); local builds say 0.1.0.
pub const VERSION: &str = match option_env!("BLAZE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Parser)]
#[command(name = "blaze", version = VERSION, about = "Blaze IDE in the terminal: chat with BlazeAPI models and let the agent work on your project.")]
struct Cli {
    /// Run one prompt without the full-screen UI and print the answer.
    #[arg(short = 'p', long = "print", value_name = "PROMPT")]
    print: Option<String>,
    /// Project folder (default: the current folder).
    #[arg(short = 'C', long = "dir", global = true)]
    dir: Option<PathBuf>,
    /// Model id (see `blaze models`).
    #[arg(short, long, global = true)]
    model: Option<String>,
    /// ask | plan | agent
    #[arg(long, global = true)]
    mode: Option<String>,
    /// What the agent may do without asking: ask | edits | full.
    #[arg(short, long, global = true)]
    approval: Option<String>,
    /// With -p: print events as JSON lines.
    #[arg(long)]
    json: bool,
    /// Resume a saved chat by id (see `blaze chats`).
    #[arg(short, long)]
    resume: Option<String>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Sign in with BlazeAPI in the browser (or paste a key with --key).
    Login {
        #[arg(long)]
        key: bool,
    },
    /// Sign out and revoke this app's key.
    Logout,
    /// Show the signed-in account and balance.
    Whoami,
    /// List the models your account can use.
    Models {
        /// Filter by text.
        filter: Option<String>,
    },
    /// List saved chats of this project (shared with Blaze IDE).
    Chats,
    /// Continue on mobile: let your phone continue chats on this computer.
    Serve {
        /// Same Wi-Fi: listen on the local network (plain http).
        #[arg(long)]
        lan: bool,
        /// A free Cloudflare quick tunnel (random https address).
        #[arg(long)]
        tunnel: bool,
        /// The BlazeAPI relay: works from anywhere, no setup.
        #[arg(long)]
        relay: bool,
        /// Your own domain pointed at this computer (https://ide.example.com).
        #[arg(long)]
        domain: Option<String>,
        /// Port for the phone server (default 47900).
        #[arg(long)]
        port: Option<u16>,
    },
    /// BlBots: your AI bots, their routines and rooms.
    Bots {
        #[command(subcommand)]
        cmd: bots::BotsCmd,
    },
    /// Manage Continue on mobile: password, phones, domain tunnel token.
    Mobile {
        #[command(subcommand)]
        cmd: MobileCmd,
    },
}

#[derive(Subcommand)]
enum MobileCmd {
    /// Set or change the phone password (signs every phone out).
    Password,
    /// List signed-in phones.
    Devices,
    /// Sign one phone out.
    Revoke { id: String },
    /// Turn Continue on mobile off and sign every phone out.
    Off,
    /// Save the Cloudflare tunnel token for --domain (read from stdin).
    DomainToken,
}

fn project_dir(cli: &Cli) -> Result<PathBuf, String> {
    let d = cli.dir.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    blaze_core::agent::project_root(&d).map_err(|e| e.to_string())
}

fn approval_of(cli: &Cli, s: &Session) -> Result<Approval, String> {
    match &cli.approval {
        Some(a) => Approval::parse(a).ok_or_else(|| format!("Unknown approval level {a}: use ask, edits or full")),
        None => Ok(s.default_approval()),
    }
}

fn mode_of(cli: &Cli) -> Result<String, String> {
    let m = cli.mode.clone().unwrap_or_else(|| "agent".into());
    if matches!(m.as_str(), "ask" | "plan" | "agent") { Ok(m) } else { Err(format!("Unknown mode {m}: use ask, plan or agent")) }
}

async fn login(s: &mut Session, paste: bool) -> Result<(), String> {
    // A server without a desktop cannot finish the browser flow (the browser
    // must reach 127.0.0.1 on this machine), so it pastes a key instead.
    let headless = cfg!(target_os = "linux") && std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none();
    if paste || headless {
        eprint!("Paste a BlazeAPI key (from blazeapi.org > Dashboard > Keys): ");
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
        let key = line.trim().to_string();
        if key.is_empty() {
            return Err("No key given".into());
        }
        let bal = blaze_core::models::fetch_balance(&s.http, &s.v1_base, &key).await.map_err(|e| format!("That key did not work: {e}"))?;
        config::save_key(&key)?;
        let _ = s.store.set_account(None);
        s.key = Some(key);
        println!("Signed in with a pasted key. Balance: ${bal:.2}");
        return Ok(());
    }
    let cancel = tokio_util::sync::CancellationToken::new();
    let signed = blaze_core::auth::sign_in(
        &s.http,
        &s.api_base,
        |url| {
            eprintln!("Opening your browser to sign in. If it does not open, visit:\n  {url}\n");
            let _ = open_browser(url);
            Ok(())
        },
        cancel,
    )
    .await
    .map_err(|e| format!("Sign-in failed: {e}"))?;
    config::save_key(&signed.api_key)?;
    s.store.set_account(Some(json!({ "email": signed.email, "username": signed.username }))).map_err(|e| e.to_string())?;
    s.key = Some(signed.api_key);
    println!("Signed in as {}.", if signed.email.is_empty() { signed.username } else { signed.email });
    Ok(())
}

pub fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    r.map(|_| ())
}

async fn run(mut cli: Cli) -> Result<(), String> {
    let mut s = Session::open();
    if matches!(cli.cmd, Some(Cmd::Bots { .. })) {
        if let Some(Cmd::Bots { cmd }) = cli.cmd.take() {
            return bots::run(&mut s, cmd).await;
        }
    }
    match &cli.cmd {
        Some(Cmd::Login { key }) => return login(&mut s, *key).await,
        Some(Cmd::Logout) => {
            if let Some(k) = s.key.clone() {
                blaze_core::auth::revoke_key(&s.http, &s.api_base, &k).await;
            }
            config::delete_key();
            let _ = s.store.set_account(None);
            println!("Signed out.");
            return Ok(());
        }
        Some(Cmd::Whoami) => {
            let Some(k) = s.key.clone() else { return Err("Not signed in. Run `blaze login`.".into()) };
            let bal = blaze_core::models::fetch_balance(&s.http, &s.v1_base, &k).await.map_err(|e| e.to_string())?;
            println!("{}  ·  balance ${bal:.2}", s.account_name().unwrap_or_else(|| "signed in with a key".into()));
            return Ok(());
        }
        Some(Cmd::Models { filter }) => {
            s.load_models(true).await?;
            let f = filter.clone().unwrap_or_default().to_lowercase();
            for m in s.models.iter().filter(|m| m.kind == "chat" && (f.is_empty() || m.id.to_lowercase().contains(&f) || m.display_name.to_lowercase().contains(&f))) {
                println!("{:<44} {:<28} {}", m.id, m.display_name, session::price_text(m));
            }
            return Ok(());
        }
        Some(Cmd::Chats) => {
            let root = project_dir(&cli)?;
            let list = blaze_core::chats::list(&root);
            if list.is_empty() {
                println!("No saved chats in {}.", root.display());
            }
            for c in list {
                println!("{:<16} {:>4} msgs  {}", c.id, c.message_count, c.title);
            }
            return Ok(());
        }
        Some(Cmd::Serve { lan, tunnel, relay, domain, port }) => {
            let opts = serve::ServeOpts { lan: *lan, tunnel: *tunnel, relay: *relay, domain: domain.clone(), domain_token: false, port: *port };
            if let Ok(root) = project_dir(&cli) {
                if root.join(".git").exists() || root.join(".blazeide").exists() {
                    let _ = s.store.add_recent_project(&root.display().to_string());
                }
            }
            return serve::serve(&mut s, opts).await;
        }
        Some(Cmd::Bots { .. }) => unreachable!("handled above"),
        Some(Cmd::Mobile { cmd }) => {
            use blaze_core::remote::auth;
            return match cmd {
                MobileCmd::Password => serve::set_password_interactive(&s.store),
                MobileCmd::Devices => {
                    serve::devices(&s.store);
                    Ok(())
                }
                MobileCmd::Revoke { id } => match auth::revoke_device(&s.store, id) {
                    Ok(true) => {
                        println!("Signed out {id}.");
                        Ok(())
                    }
                    Ok(false) => Err(format!("No phone {id}. See `blaze mobile devices`.")),
                    Err(e) => Err(e.to_string()),
                },
                MobileCmd::Off => auth::update(&s.store, |c| {
                    c.enabled = false;
                    c.devices.clear();
                    Ok(())
                })
                .map(|_| println!("Continue on mobile is off; every phone was signed out."))
                .map_err(|e| e.to_string()),
                MobileCmd::DomainToken => {
                    eprint!("Paste the Cloudflare tunnel token (Zero Trust > Networks > Tunnels > your tunnel > Install): ");
                    let mut t = String::new();
                    std::io::stdin().read_line(&mut t).map_err(|e| e.to_string())?;
                    let t = t.trim();
                    if t.len() < 40 {
                        return Err("That does not look like a tunnel token.".into());
                    }
                    config::save_secret("mobile-tunnel-token", t)?;
                    auth::update(&s.store, |c| {
                        c.domain_tunnel = true;
                        Ok(())
                    })
                    .map_err(|e| e.to_string())?;
                    println!("Saved. Now run: blaze serve --domain https://your.domain");
                    Ok(())
                }
            };
        }
        None => {}
    }
    let root = project_dir(&cli)?;
    let approval = approval_of(&cli, &s)?;
    let mode = mode_of(&cli)?;
    let model = cli.model.clone().unwrap_or_else(|| s.default_model());
    if let Some(prompt) = cli.print.clone() {
        let prompt = if prompt == "-" {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf).map_err(|e| e.to_string())?;
            buf
        } else {
            prompt
        };
        return oneshot::run(&mut s, &root, &model, &mode, approval, &prompt, cli.json, cli.resume.as_deref()).await;
    }
    if !std::io::stdout().is_terminal() {
        return Err("The full-screen chat needs a terminal. Use `blaze -p \"...\"` in scripts.".into());
    }
    let _ = s.store.add_recent_project(&root.display().to_string());
    tui::run(s, root, model, mode, approval, cli.resume.clone()).await
}

#[tokio::main]
async fn main() {
    // `blaze models | head`: exit quietly when the pipe closes (Rust ignores
    // SIGPIPE by default, which turns it into a panic on the next print).
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("blaze: {e}");
        std::process::exit(1);
    }
}
