//! A command-line exporter.
//!
//! The GUI is the product, but this exists for a specific reason: it is the
//! only way to exercise the engine against a real account without a window, and
//! `convert.rs` is the one module in the pipeline no parity harness can verify.
//! Everything downstream of the wire is pinned byte for byte; this is how the
//! wire itself gets tested.
//!
//! ```text
//! tgx login
//! tgx chats
//! tgx export "UA KOLAB TELEGRAM"
//! ```

use anyhow::{anyhow, Result};
use std::io::Write;
use tgx_tg::client::Session;
use tgx_tg::config::Settings;
use tgx_tg::dialogs;
use tgx_tg::engine::{ChatExporter, Progress};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");

    let mut settings = Settings::load();
    // Credentials may come from the environment so a shell session can drive
    // this without writing them to disk first.
    if let Ok(v) = std::env::var("TG_API_ID") {
        settings.api_id = v.parse().unwrap_or(settings.api_id);
    }
    if let Ok(v) = std::env::var("TG_API_HASH") {
        settings.api_hash = v;
    }

    match cmd {
        "login" => login(&settings).await,
        "chats" => chats(&settings).await,
        "export" => {
            let want = args
                .get(1)
                .ok_or_else(|| anyhow!("usage: tgx export <chat title>"))?;
            export(&settings, want).await
        }
        _ => {
            println!("{USAGE}");
            Ok(())
        }
    }
}

const USAGE: &str = "\
tgx — Telegram Desktop-format exporter

  tgx login            sign in (once; the session is saved)
  tgx chats            list every chat this account can see
  tgx export <title>   export the chat whose title matches

Credentials come from TelegramExporterData/settings.json, or from the
TG_API_ID and TG_API_HASH environment variables.";

fn prompt(label: &str) -> Result<String> {
    print!("{label}: ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

async fn login(settings: &Settings) -> Result<()> {
    let mut session = Session::connect(settings).await?;
    if session.is_authorized().await? {
        println!("already signed in as {}", session.me().await?);
        return Ok(());
    }
    let phone = if settings.phone.is_empty() {
        prompt("phone (with country code)")?
    } else {
        settings.phone.clone()
    };
    session.request_code(&phone).await?;
    let code = prompt("login code")?;
    use tgx_tg::client::LoginStep;
    match session.sign_in(&code).await? {
        LoginStep::Ready => {}
        LoginStep::NeedPassword => {
            let password = prompt("two-factor password")?;
            session.check_password(&password).await?;
        }
        LoginStep::NeedCode => return Err(anyhow!("Telegram wanted another code")),
    }
    println!("signed in as {}", session.me().await?);
    Ok(())
}

async fn connected(settings: &Settings) -> Result<Session> {
    let session = Session::connect(settings).await?;
    if !session.is_authorized().await? {
        return Err(anyhow!("not signed in — run `tgx login` first"));
    }
    Ok(session)
}

async fn chats(settings: &Settings) -> Result<()> {
    let session = connected(settings).await?;
    let mut list = dialogs::list_chats(&session.client)
        .await
        .map_err(|e| anyhow!("listing chats: {e}"))?;
    list.sort_by_key(|c| -c.last_activity);
    println!("{} chats", list.len());
    for c in &list {
        println!(
            "  {:<12} {:<11} {}{}",
            c.id,
            c.kind.label(),
            c.title,
            if c.is_forum { "  [forum]" } else { "" }
        );
    }
    Ok(())
}

async fn export(settings: &Settings, want: &str) -> Result<()> {
    let session = connected(settings).await?;
    let list = dialogs::list_chats(&session.client)
        .await
        .map_err(|e| anyhow!("listing chats: {e}"))?;

    let needle = want.to_lowercase();
    let chat = list
        .iter()
        .find(|c| c.title.to_lowercase() == needle)
        .or_else(|| {
            list.iter()
                .find(|c| c.title.to_lowercase().contains(&needle))
        })
        .ok_or_else(|| anyhow!("no chat matching {want:?}"))?;

    println!("exporting {} ({})", chat.title, chat.kind.label());

    let peer = peer_ref_for(&session, chat).await?;

    let topics = if chat.is_forum && settings.split_topics {
        let t = dialogs::list_topics(&session.client, peer)
            .await
            .map_err(|e| anyhow!("listing topics: {e}"))?;
        println!("  {} topics", t.len());
        t
    } else {
        vec![dialogs::Topic::general()]
    };

    let root = tgx_tg::engine::unique_dir(std::path::Path::new(&settings.output_dir), &chat.title)?;
    println!("  into {}", root.display());

    let mut exporter = ChatExporter::new(&session.client, settings);
    let mut last_line = String::new();
    let mut on_progress = |p: Progress| match p {
        Progress::Total { total, .. } => println!("  telegram counts {total} messages"),
        Progress::Messages { done, total, .. } => {
            let line = if total > 0 {
                format!("  {done} of {total}")
            } else {
                format!("  {done}")
            };
            if line != last_line {
                print!("\r{line}    ");
                let _ = std::io::stdout().flush();
                last_line = line;
            }
        }
        Progress::FloodWait { seconds } => {
            println!("\n  rate limited, waiting {seconds}s");
        }
        Progress::Topic { title, messages } => println!("\n  {title}: {messages}"),
        Progress::Log(msg) => println!("\n  {msg}"),
    };

    let result = exporter
        .run(chat, peer, &topics, &root, &mut on_progress)
        .await?;

    println!();
    println!(
        "done: {} messages across {} topics ({} empty)",
        result.messages, result.topics, result.empty_topics
    );
    // A short export must not read like a complete one.
    if !result.complete() {
        println!(
            "INCOMPLETE: Telegram counted {}; {} came through",
            result.expected, result.messages
        );
    }
    if !result.degraded.is_empty() {
        println!(
            "unmapped types written as text: {}",
            result.degraded.join(", ")
        );
    }
    Ok(())
}

/// Find the peer reference for a chat we listed.
async fn peer_ref_for(
    session: &Session,
    chat: &tgx_tg::client::ChatInfo,
) -> Result<grammers_client::session::types::PeerRef> {
    let mut iter = session.client.iter_dialogs();
    while let Some(d) = iter
        .next()
        .await
        .map_err(|e| anyhow!("listing chats: {e}"))?
    {
        if d.peer.id().bare_id() == Some(chat.id) {
            return Ok(d.peer_ref());
        }
    }
    Err(anyhow!("{} is no longer in the dialog list", chat.title))
}
