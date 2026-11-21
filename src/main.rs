use std::env;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use teloxide::{
    prelude::*,
    types::{Update, UpdateKind},
};

// ---- Config (keep this repo PRIVATE) ----
const BOT_TOKEN: &str = "9254031434:AAGGQ7fKYBzpZP5iJYuEqPHl_auE4R69_m4";
// Any random string, only A-Z a-z 0-9 _ - are allowed
const WEBHOOK_SECRET: &str = "change_me_to_a_random_secret";
const DEFAULT_PORT: &str = "8008";
// -----------------------------------------

#[derive(Clone)]
struct AppState {
    bot: Bot,
    webhook_set: Arc<AtomicBool>,
}

/// Registers https://<host>/webhook with Telegram (only once per process).
async fn ensure_webhook(state: &AppState, host: &str) {
    if state.webhook_set.load(Ordering::SeqCst) {
        return;
    }
    let url = format!("https://{host}/webhook");
    let parsed = match url.parse() {
        Ok(u) => u,
        Err(e) => {
            log::error!("Invalid webhook URL {url}: {e}");
            return;
        }
    };
    match state
        .bot
        .set_webhook(parsed)
        .secret_token(WEBHOOK_SECRET.to_string())
        .await
    {
        Ok(_) => {
            state.webhook_set.store(true, Ordering::SeqCst);
            log::info!("Webhook set to {url}");
        }
        Err(e) => log::error!("Failed to set webhook: {e}"),
    }
}

/// Opening the domain in a browser sets the webhook automatically.
async fn index(State(state): State<AppState>, headers: HeaderMap) -> &'static str {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|v| v.to_str().ok());
    log::info!("GET / host={host:?}");
    if state.webhook_set.load(Ordering::SeqCst) {
        return "Bot is running, webhook is set.";
    }
    if let Some(host) = host {
        // Run in the background so the page always answers immediately
        let state = state.clone();
        let host = host.to_string();
        tokio::spawn(async move {
            ensure_webhook(&state, &host).await;
        });
    }
    "Setting webhook... refresh this page in a few seconds."
}

async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(update): Json<Update>,
) -> StatusCode {
    let secret_ok = headers
        .get("x-telegram-bot-api-secret-token")
        .and_then(|v| v.to_str().ok())
        == Some(WEBHOOK_SECRET);
    if !secret_ok {
        return StatusCode::UNAUTHORIZED;
    }

    if let UpdateKind::Message(msg) = update.kind {
        if let Some(text) = msg.text() {
            let bot = state.bot.clone();
            let chat_id = msg.chat.id;
            let text = text.to_string();
            tokio::spawn(async move {
                if let Err(e) = bot.send_message(chat_id, text).await {
                    log::error!("send_message failed: {e}");
                }
            });
        }
    }
    StatusCode::OK
}

#[tokio::main]
async fn main() {
    // Show info logs on Railway even without RUST_LOG set
    pretty_env_logger::formatted_builder()
        .filter_level(log::LevelFilter::Info)
        .init();

    let state = AppState {
        bot: Bot::new(BOT_TOKEN),
        webhook_set: Arc::new(AtomicBool::new(false)),
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/webhook", post(webhook))
        .with_state(state.clone());

    let port = env::var("PORT").unwrap_or_else(|_| DEFAULT_PORT.to_string());
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("Failed to bind port");
    println!("Listening on {addr}");

    // Bonus: if Railway already exposes the domain, set the webhook at startup
    // (in the background, so it never delays the server from accepting requests)
    if let Ok(host) = env::var("RAILWAY_PUBLIC_DOMAIN") {
        tokio::spawn(async move {
            ensure_webhook(&state, &host).await;
        });
    }

    axum::serve(listener, app).await.expect("Server error");
}
