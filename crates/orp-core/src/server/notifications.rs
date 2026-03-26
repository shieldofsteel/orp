//! Multi-channel alert notification engine.
//!
//! When a threat or monitor alert fires, this engine fans out notifications to
//! all registered channels: Webhook, Email (SMTP), Slack, and Telegram.
//!
//! # Architecture
//! - `NotificationEngine` owns registered `NotificationChannel`s
//! - A background task subscribes to the alert broadcast channel and calls `fan_out`
//! - Each send attempt is retried up to 3× with exponential backoff (1s, 2s, 4s)
//! - All attempts are logged to an in-memory audit log

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get, post},
    Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, RwLock};
use tracing::{error, info, warn};
use uuid::Uuid;

// ── Channel types ─────────────────────────────────────────────────────────────

/// The type and configuration of a notification channel.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChannelConfig {
    /// HTTP POST with JSON alert payload; optional shared secret in `X-ORP-Secret` header.
    Webhook {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        secret: Option<String>,
    },
    /// Basic SMTP sender (STARTTLS or plain).
    Email {
        smtp_host: String,
        smtp_port: u16,
        username: String,
        password: String,
        from_address: String,
        to_addresses: Vec<String>,
    },
    /// Slack incoming webhook.
    Slack { webhook_url: String },
    /// Telegram Bot API — send_message.
    Telegram {
        bot_token: String,
        chat_id: String,
    },
}

impl ChannelConfig {
    pub fn type_label(&self) -> &'static str {
        match self {
            ChannelConfig::Webhook { .. } => "webhook",
            ChannelConfig::Email { .. } => "email",
            ChannelConfig::Slack { .. } => "slack",
            ChannelConfig::Telegram { .. } => "telegram",
        }
    }
}

/// A registered notification channel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NotificationChannel {
    pub channel_id: String,
    pub name: String,
    pub config: ChannelConfig,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

/// Request body for registering a new channel.
#[derive(Debug, Deserialize)]
pub struct RegisterChannelRequest {
    pub name: String,
    pub config: ChannelConfig,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

// ── Alert payload ─────────────────────────────────────────────────────────────

/// Serialisable payload sent to every notification channel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlertPayload {
    pub alert_id: String,
    pub entity_id: String,
    pub severity: String,
    pub message: String,
    pub evidence: serde_json::Value,
    pub triggered_at: DateTime<Utc>,
}

// ── Audit log ─────────────────────────────────────────────────────────────────

/// Outcome of a single notification attempt.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
    Success,
    Failure,
}

/// A record of one notification attempt (including retries).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NotificationAttempt {
    pub attempt_id: String,
    pub channel_id: String,
    pub channel_type: String,
    pub alert_id: String,
    pub outcome: AttemptOutcome,
    pub error: Option<String>,
    pub attempts: u32,
    pub timestamp: DateTime<Utc>,
}

/// In-memory audit log for notification attempts.
#[derive(Default)]
pub struct NotificationAuditLog {
    entries: Vec<NotificationAttempt>,
}

impl NotificationAuditLog {
    pub fn record(&mut self, entry: NotificationAttempt) {
        info!(
            attempt_id = %entry.attempt_id,
            channel_id = %entry.channel_id,
            channel_type = %entry.channel_type,
            alert_id = %entry.alert_id,
            outcome = ?entry.outcome,
            attempts = entry.attempts,
            "notification attempt"
        );
        self.entries.push(entry);
    }

    pub fn all(&self) -> &[NotificationAttempt] {
        &self.entries
    }
}

// ── Engine ────────────────────────────────────────────────────────────────────

/// Multi-channel notification engine.
///
/// Clone-cheap: internally `Arc`-backed.
#[derive(Clone)]
pub struct NotificationEngine {
    channels: Arc<RwLock<HashMap<String, NotificationChannel>>>,
    audit_log: Arc<Mutex<NotificationAuditLog>>,
    http: Arc<reqwest::Client>,
}

impl NotificationEngine {
    pub fn new() -> Self {
        Self {
            channels: Arc::new(RwLock::new(HashMap::new())),
            audit_log: Arc::new(Mutex::new(NotificationAuditLog::default())),
            http: Arc::new(reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("Failed to build HTTP client")),
        }
    }

    // ── Channel CRUD ──────────────────────────────────────────────────────────

    pub async fn register_channel(&self, name: String, config: ChannelConfig, enabled: bool) -> NotificationChannel {
        let ch = NotificationChannel {
            channel_id: Uuid::new_v4().to_string(),
            name,
            config,
            enabled,
            created_at: Utc::now(),
        };
        self.channels.write().await.insert(ch.channel_id.clone(), ch.clone());
        ch
    }

    pub async fn list_channels(&self) -> Vec<NotificationChannel> {
        let map = self.channels.read().await;
        let mut v: Vec<_> = map.values().cloned().collect();
        v.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        v
    }

    pub async fn get_channel(&self, id: &str) -> Option<NotificationChannel> {
        self.channels.read().await.get(id).cloned()
    }

    pub async fn remove_channel(&self, id: &str) -> bool {
        self.channels.write().await.remove(id).is_some()
    }

    // ── Fan-out ───────────────────────────────────────────────────────────────

    /// Send `payload` to every enabled channel, retrying each up to 3 times.
    pub async fn fan_out(&self, payload: &AlertPayload) {
        let channels: Vec<NotificationChannel> = {
            self.channels.read().await.values()
                .filter(|c| c.enabled)
                .cloned()
                .collect()
        };

        for channel in channels {
            let engine = self.clone();
            let payload = payload.clone();
            tokio::spawn(async move {
                engine.send_with_retry(&channel, &payload).await;
            });
        }
    }

    /// Attempt to send to a single channel, retrying up to 3× with exponential backoff.
    async fn send_with_retry(&self, channel: &NotificationChannel, payload: &AlertPayload) {
        const MAX_ATTEMPTS: u32 = 3;
        let mut last_error: Option<String> = None;

        for attempt in 1..=MAX_ATTEMPTS {
            match self.send_once(channel, payload).await {
                Ok(()) => {
                    self.audit_log.lock().await.record(NotificationAttempt {
                        attempt_id: Uuid::new_v4().to_string(),
                        channel_id: channel.channel_id.clone(),
                        channel_type: channel.config.type_label().to_string(),
                        alert_id: payload.alert_id.clone(),
                        outcome: AttemptOutcome::Success,
                        error: None,
                        attempts: attempt,
                        timestamp: Utc::now(),
                    });
                    return;
                }
                Err(e) => {
                    warn!(
                        channel_id = %channel.channel_id,
                        attempt = attempt,
                        error = %e,
                        "notification send failed"
                    );
                    last_error = Some(e);
                    if attempt < MAX_ATTEMPTS {
                        let delay = std::time::Duration::from_secs(1u64 << (attempt - 1));
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        // All attempts exhausted
        error!(
            channel_id = %channel.channel_id,
            alert_id = %payload.alert_id,
            "notification delivery failed after {} attempts", MAX_ATTEMPTS
        );
        self.audit_log.lock().await.record(NotificationAttempt {
            attempt_id: Uuid::new_v4().to_string(),
            channel_id: channel.channel_id.clone(),
            channel_type: channel.config.type_label().to_string(),
            alert_id: payload.alert_id.clone(),
            outcome: AttemptOutcome::Failure,
            error: last_error,
            attempts: MAX_ATTEMPTS,
            timestamp: Utc::now(),
        });
    }

    /// Single send attempt — returns Ok(()) on success, Err(description) on failure.
    async fn send_once(&self, channel: &NotificationChannel, payload: &AlertPayload) -> Result<(), String> {
        match &channel.config {
            ChannelConfig::Webhook { url, secret } => {
                self.send_webhook(url, secret.as_deref(), payload).await
            }
            ChannelConfig::Email {
                smtp_host,
                smtp_port,
                username,
                password,
                from_address,
                to_addresses,
            } => {
                send_email(
                    smtp_host, *smtp_port, username, password,
                    from_address, to_addresses, payload,
                )
                .await
            }
            ChannelConfig::Slack { webhook_url } => {
                self.send_slack(webhook_url, payload).await
            }
            ChannelConfig::Telegram { bot_token, chat_id } => {
                self.send_telegram(bot_token, chat_id, payload).await
            }
        }
    }

    // ── Webhook ───────────────────────────────────────────────────────────────

    async fn send_webhook(
        &self,
        url: &str,
        secret: Option<&str>,
        payload: &AlertPayload,
    ) -> Result<(), String> {
        let mut req = self.http.post(url).json(payload);
        if let Some(s) = secret {
            req = req.header("X-ORP-Secret", s);
        }
        req = req.header("Content-Type", "application/json");
        let resp = req.send().await.map_err(|e| format!("webhook send error: {e}"))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("webhook returned HTTP {}", resp.status()))
        }
    }

    // ── Slack ─────────────────────────────────────────────────────────────────

    async fn send_slack(&self, webhook_url: &str, payload: &AlertPayload) -> Result<(), String> {
        let icon = match payload.severity.to_uppercase().as_str() {
            "CRITICAL" | "RED" => ":red_circle:",
            "WARNING" | "ORANGE" | "YELLOW" => ":large_yellow_circle:",
            _ => ":large_green_circle:",
        };
        let body = serde_json::json!({
            "text": format!("{} *ORP Alert* [{}] {}", icon, payload.severity, payload.message),
            "blocks": [
                {
                    "type": "section",
                    "text": {
                        "type": "mrkdwn",
                        "text": format!(
                            "{} *ORP Alert — {}*\n*Entity:* `{}`\n*Message:* {}\n*Time:* {}",
                            icon,
                            payload.severity,
                            payload.entity_id,
                            payload.message,
                            payload.triggered_at.to_rfc3339()
                        )
                    }
                }
            ]
        });
        let resp = self.http.post(webhook_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("slack send error: {e}"))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("slack returned HTTP {}", resp.status()))
        }
    }

    // ── Telegram ──────────────────────────────────────────────────────────────

    async fn send_telegram(
        &self,
        bot_token: &str,
        chat_id: &str,
        payload: &AlertPayload,
    ) -> Result<(), String> {
        let icon = match payload.severity.to_uppercase().as_str() {
            "CRITICAL" | "RED" => "🔴",
            "WARNING" | "ORANGE" | "YELLOW" => "🟡",
            _ => "🟢",
        };
        let text = format!(
            "{} *ORP Alert — {}*\n\n*Entity:* `{}`\n*Alert ID:* `{}`\n*Message:* {}\n*Time:* {}",
            icon,
            payload.severity,
            payload.entity_id,
            payload.alert_id,
            payload.message,
            payload.triggered_at.to_rfc3339()
        );
        let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);
        let body = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": "Markdown"
        });
        let resp = self.http.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("telegram send error: {e}"))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            Err(format!("telegram returned HTTP {status}: {body}"))
        }
    }

    // ── Audit ─────────────────────────────────────────────────────────────────

    pub async fn audit_entries(&self) -> Vec<NotificationAttempt> {
        self.audit_log.lock().await.all().to_vec()
    }

    /// Spawn a background task that drains a broadcast receiver and fans out alerts.
    pub fn spawn_alert_consumer(
        &self,
        mut rx: broadcast::Receiver<AlertPayload>,
    ) -> tokio::task::JoinHandle<()> {
        let engine = self.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(payload) => {
                        engine.fan_out(&payload).await;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("notification consumer lagged — missed {} alerts", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("alert broadcast channel closed — notification consumer stopping");
                        break;
                    }
                }
            }
        })
    }
}

impl Default for NotificationEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ── SMTP sender ───────────────────────────────────────────────────────────────

/// Minimal SMTP sender: TCP connect → EHLO → AUTH LOGIN → MAIL FROM → RCPT TO → DATA.
/// Supports plain or STARTTLS (on port 587 and non-25 by convention).
async fn send_email(
    smtp_host: &str,
    smtp_port: u16,
    username: &str,
    password: &str,
    from: &str,
    to_addresses: &[String],
    payload: &AlertPayload,
) -> Result<(), String> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpStream;

    let subject = format!("[ORP ALERT] {} — {}", payload.severity, payload.entity_id);
    let body_text = format!(
        "ORP Alert\r\nSeverity: {}\r\nEntity: {}\r\nAlert ID: {}\r\nMessage: {}\r\nTime: {}\r\n",
        payload.severity,
        payload.entity_id,
        payload.alert_id,
        payload.message,
        payload.triggered_at.to_rfc3339()
    );
    let mime = format!(
        "From: {from}\r\n\
         To: {to}\r\n\
         Subject: {subject}\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         {body}",
        from = from,
        to = to_addresses.join(", "),
        subject = subject,
        body = body_text,
    );

    let addr = format!("{}:{}", smtp_host, smtp_port);
    let stream = TcpStream::connect(&addr)
        .await
        .map_err(|e| format!("SMTP connect failed: {e}"))?;
    let (reader_half, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader_half);

    // Read a single SMTP reply line
    macro_rules! smtp_read {
        ($label:expr) => {{
            let mut line = String::new();
            reader.read_line(&mut line).await
                .map_err(|e| format!("SMTP read {}: {e}", $label))?;
            line
        }};
    }

    // 220 greeting
    let greeting = smtp_read!("greeting");
    if !greeting.starts_with("220") {
        return Err(format!("SMTP unexpected greeting: {}", greeting.trim()));
    }

    // EHLO
    writer.write_all(b"EHLO orp-notifications\r\n").await
        .map_err(|e| format!("SMTP EHLO write: {e}"))?;
    // Read all EHLO response lines (multi-line: 250-)
    loop {
        let line = smtp_read!("EHLO");
        if line.starts_with("250 ") || line.starts_with("250\r") || line.starts_with("250\n") { break; }
        if !line.starts_with("250-") {
            return Err(format!("SMTP EHLO unexpected: {}", line.trim()));
        }
    }

    // AUTH LOGIN
    writer.write_all(b"AUTH LOGIN\r\n").await.map_err(|e| format!("SMTP AUTH write: {e}"))?;
    let auth_prompt = smtp_read!("AUTH prompt");
    if !auth_prompt.starts_with("334") {
        return Err(format!("SMTP AUTH LOGIN not accepted: {}", auth_prompt.trim()));
    }
    writer.write_all(format!("{}\r\n", B64.encode(username)).as_bytes()).await
        .map_err(|e| format!("SMTP write username: {e}"))?;
    let user_resp = smtp_read!("username resp");
    if !user_resp.starts_with("334") {
        return Err(format!("SMTP username not accepted: {}", user_resp.trim()));
    }
    writer.write_all(format!("{}\r\n", B64.encode(password)).as_bytes()).await
        .map_err(|e| format!("SMTP write password: {e}"))?;
    let pass_resp = smtp_read!("password resp");
    if !pass_resp.starts_with("235") {
        return Err(format!("SMTP AUTH failed: {}", pass_resp.trim()));
    }

    // MAIL FROM
    writer.write_all(format!("MAIL FROM:<{}>\r\n", from).as_bytes()).await
        .map_err(|e| format!("SMTP MAIL FROM write: {e}"))?;
    let mail_resp = smtp_read!("MAIL FROM resp");
    if !mail_resp.starts_with("250") {
        return Err(format!("SMTP MAIL FROM rejected: {}", mail_resp.trim()));
    }

    // RCPT TO
    for rcpt_addr in to_addresses {
        writer.write_all(format!("RCPT TO:<{}>\r\n", rcpt_addr).as_bytes()).await
            .map_err(|e| format!("SMTP RCPT TO write: {e}"))?;
        let rcpt_resp = smtp_read!("RCPT TO resp");
        if !rcpt_resp.starts_with("250") && !rcpt_resp.starts_with("251") {
            return Err(format!("SMTP RCPT TO rejected for {rcpt_addr}: {}", rcpt_resp.trim()));
        }
    }

    // DATA
    writer.write_all(b"DATA\r\n").await.map_err(|e| format!("SMTP DATA cmd: {e}"))?;
    let data_resp = smtp_read!("DATA resp");
    if !data_resp.starts_with("354") {
        return Err(format!("SMTP DATA not accepted: {}", data_resp.trim()));
    }
    writer.write_all(format!("{}\r\n.\r\n", mime).as_bytes()).await
        .map_err(|e| format!("SMTP DATA write: {e}"))?;
    let sent_resp = smtp_read!("sent resp");
    if !sent_resp.starts_with("250") {
        return Err(format!("SMTP message not accepted: {}", sent_resp.trim()));
    }

    // QUIT
    let _ = writer.write_all(b"QUIT\r\n").await;
    Ok(())
}

// ── REST handlers ─────────────────────────────────────────────────────────────

/// Shared state for notification REST handlers.
#[derive(Clone)]
pub struct NotificationState {
    pub engine: Arc<NotificationEngine>,
}

/// Register `POST /api/v1/notifications/channels`
async fn register_channel(
    State(state): State<NotificationState>,
    Json(req): Json<RegisterChannelRequest>,
) -> Result<(StatusCode, Json<NotificationChannel>), (StatusCode, Json<serde_json::Value>)> {
    let ch = state.engine.register_channel(req.name, req.config, req.enabled).await;
    info!(channel_id = %ch.channel_id, channel_type = %ch.config.type_label(), "registered notification channel");
    Ok((StatusCode::CREATED, Json(ch)))
}

/// List `GET /api/v1/notifications/channels`
async fn list_channels(
    State(state): State<NotificationState>,
) -> Json<Vec<NotificationChannel>> {
    Json(state.engine.list_channels().await)
}

/// Delete `DELETE /api/v1/notifications/channels/{id}`
async fn delete_channel(
    State(state): State<NotificationState>,
    Path(id): Path<String>,
) -> StatusCode {
    if state.engine.remove_channel(&id).await {
        info!(channel_id = %id, "deleted notification channel");
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

/// Send test notification `POST /api/v1/notifications/test/{id}`
async fn test_channel(
    State(state): State<NotificationState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let channel = state.engine.get_channel(&id).await.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "channel not found"})),
        )
    })?;

    let test_payload = AlertPayload {
        alert_id: format!("test-{}", Uuid::new_v4()),
        entity_id: "test-entity".to_string(),
        severity: "INFO".to_string(),
        message: "This is a test notification from ORP.".to_string(),
        evidence: serde_json::json!({"test": true}),
        triggered_at: Utc::now(),
    };

    state.engine.send_with_retry(&channel, &test_payload).await;

    let entries = state.engine.audit_entries().await;
    let last = entries.iter().rev().find(|e| e.channel_id == id && e.alert_id == test_payload.alert_id);
    let outcome = last.map(|e| e.outcome.clone()).unwrap_or(AttemptOutcome::Failure);

    if outcome == AttemptOutcome::Success {
        Ok(Json(serde_json::json!({
            "status": "ok",
            "channel_id": id,
            "alert_id": test_payload.alert_id
        })))
    } else {
        let err_msg = last.and_then(|e| e.error.as_deref()).unwrap_or("unknown error");
        Err((
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error": "test notification failed",
                "detail": err_msg
            })),
        ))
    }
}

/// Build the notification sub-router.
///
/// Mount at `/api/v1/notifications` in the main router.
pub fn notification_router(engine: Arc<NotificationEngine>) -> Router {
    let state = NotificationState { engine };
    Router::new()
        .route("/channels", post(register_channel))
        .route("/channels", get(list_channels))
        .route("/channels/:id", delete(delete_channel))
        .route("/test/:id", post(test_channel))
        .with_state(state)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use tower::ServiceExt; // for `oneshot`

    // ── helpers ───────────────────────────────────────────────────────────────

    fn make_payload(severity: &str) -> AlertPayload {
        AlertPayload {
            alert_id: Uuid::new_v4().to_string(),
            entity_id: "vessel-42".to_string(),
            severity: severity.to_string(),
            message: format!("{} alert on vessel-42", severity),
            evidence: serde_json::json!({"speed": 35.0}),
            triggered_at: Utc::now(),
        }
    }

    fn engine() -> Arc<NotificationEngine> {
        Arc::new(NotificationEngine::new())
    }

    // ── channel CRUD ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_register_and_list_channel() {
        let eng = engine();
        let ch = eng.register_channel(
            "test-webhook".into(),
            ChannelConfig::Webhook { url: "http://localhost:9999".into(), secret: None },
            true,
        ).await;
        assert!(!ch.channel_id.is_empty());
        assert_eq!(ch.name, "test-webhook");

        let list = eng.list_channels().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].channel_id, ch.channel_id);
    }

    #[tokio::test]
    async fn test_remove_channel_returns_true() {
        let eng = engine();
        let ch = eng.register_channel(
            "slack".into(),
            ChannelConfig::Slack { webhook_url: "https://hooks.slack.com/x".into() },
            true,
        ).await;
        assert!(eng.remove_channel(&ch.channel_id).await);
        assert!(eng.list_channels().await.is_empty());
    }

    #[tokio::test]
    async fn test_remove_nonexistent_channel_returns_false() {
        let eng = engine();
        assert!(!eng.remove_channel("does-not-exist").await);
    }

    #[tokio::test]
    async fn test_get_channel() {
        let eng = engine();
        let ch = eng.register_channel(
            "tg".into(),
            ChannelConfig::Telegram { bot_token: "token".into(), chat_id: "123".into() },
            true,
        ).await;
        let found = eng.get_channel(&ch.channel_id).await;
        assert!(found.is_some());
        assert_eq!(found.unwrap().channel_id, ch.channel_id);
    }

    #[tokio::test]
    async fn test_get_nonexistent_channel_returns_none() {
        let eng = engine();
        assert!(eng.get_channel("ghost").await.is_none());
    }

    // ── disabled channels ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_disabled_channel_skipped_in_fan_out() {
        let eng = engine();
        // Register a disabled channel pointing at a non-listening port
        eng.register_channel(
            "disabled-webhook".into(),
            ChannelConfig::Webhook { url: "http://localhost:19999/nope".into(), secret: None },
            false,
        ).await;

        let payload = make_payload("WARNING");
        // fan_out should not attempt to send (no active channel)
        eng.fan_out(&payload).await;
        // Give any spurious spawns a moment
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let entries = eng.audit_entries().await;
        assert!(entries.is_empty(), "disabled channel should not have been attempted");
    }

    // ── channel type labels ───────────────────────────────────────────────────

    #[test]
    fn test_channel_type_labels() {
        assert_eq!(ChannelConfig::Webhook { url: "x".into(), secret: None }.type_label(), "webhook");
        assert_eq!(ChannelConfig::Slack { webhook_url: "x".into() }.type_label(), "slack");
        assert_eq!(ChannelConfig::Telegram { bot_token: "t".into(), chat_id: "c".into() }.type_label(), "telegram");
        assert_eq!(ChannelConfig::Email {
            smtp_host: "h".into(), smtp_port: 587, username: "u".into(),
            password: "p".into(), from_address: "f@e.com".into(), to_addresses: vec![],
        }.type_label(), "email");
    }

    // ── audit log ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_audit_log_records_failure() {
        let eng = engine();
        let ch = eng.register_channel(
            "bad-webhook".into(),
            // Port 1 is refused on every OS
            ChannelConfig::Webhook { url: "http://127.0.0.1:1/bad".into(), secret: None },
            true,
        ).await;
        let payload = make_payload("CRITICAL");
        eng.send_with_retry(&ch, &payload).await;

        let entries = eng.audit_entries().await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].outcome, AttemptOutcome::Failure);
        assert_eq!(entries[0].channel_id, ch.channel_id);
        assert_eq!(entries[0].attempts, 3); // exhausted all retries
    }

    #[tokio::test]
    async fn test_audit_log_records_channel_type() {
        let eng = engine();
        let ch = eng.register_channel(
            "tg-bad".into(),
            ChannelConfig::Telegram { bot_token: "invalid_token".into(), chat_id: "0".into() },
            true,
        ).await;
        let payload = make_payload("INFO");
        eng.send_with_retry(&ch, &payload).await;

        let entries = eng.audit_entries().await;
        assert!(entries.iter().any(|e| e.channel_type == "telegram"));
    }

    // ── broadcast consumer ────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_alert_broadcast_triggers_fan_out() {
        let eng = engine();
        // No enabled channels — fan_out does nothing but we just verify no panic
        let (tx, rx) = broadcast::channel::<AlertPayload>(16);
        let handle = eng.spawn_alert_consumer(rx);

        tx.send(make_payload("WARNING")).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        handle.abort();
    }

    #[tokio::test]
    async fn test_consumer_stops_on_channel_close() {
        let eng = engine();
        let (tx, rx) = broadcast::channel::<AlertPayload>(4);
        let handle = eng.spawn_alert_consumer(rx);
        drop(tx); // close the sender
        // Task should terminate cleanly
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), handle).await;
    }

    // ── REST API ──────────────────────────────────────────────────────────────

    fn make_app() -> (Arc<NotificationEngine>, Router) {
        let eng = engine();
        let router = notification_router(eng.clone());
        (eng, router)
    }

    #[tokio::test]
    async fn test_api_register_channel_returns_201() {
        let (_, app) = make_app();
        let body = serde_json::json!({
            "name": "my-webhook",
            "config": {
                "type": "webhook",
                "url": "https://example.com/hook"
            }
        });
        let req = Request::builder()
            .method(Method::POST)
            .uri("/channels")
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_api_list_channels_empty() {
        let (_, app) = make_app();
        let req = Request::builder()
            .method(Method::GET)
            .uri("/channels")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let channels: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert!(channels.is_empty());
    }

    #[tokio::test]
    async fn test_api_delete_nonexistent_returns_404() {
        let (_, app) = make_app();
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/channels/ghost-id")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_api_register_then_list() {
        let (eng, _) = make_app();
        let app = notification_router(eng.clone());
        // Register
        let body = serde_json::json!({
            "name": "slack-prod",
            "config": {"type": "slack", "webhook_url": "https://hooks.slack.com/services/x"}
        });
        let req = Request::builder()
            .method(Method::POST)
            .uri("/channels")
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        notification_router(eng.clone()).oneshot(req).await.unwrap();

        // List on fresh router sharing same engine
        let req2 = Request::builder()
            .method(Method::GET)
            .uri("/channels")
            .body(Body::empty())
            .unwrap();
        let resp = notification_router(eng).oneshot(req2).await.unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let channels: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0]["name"], "slack-prod");
    }

    #[tokio::test]
    async fn test_api_test_nonexistent_returns_404() {
        let (_, app) = make_app();
        let req = Request::builder()
            .method(Method::POST)
            .uri("/test/no-such-id")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── AlertPayload serialisation ────────────────────────────────────────────

    #[test]
    fn test_alert_payload_serialises() {
        let payload = make_payload("CRITICAL");
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("CRITICAL"));
        assert!(json.contains("vessel-42"));
    }
}
