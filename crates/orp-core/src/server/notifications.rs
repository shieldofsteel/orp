// The notification engine module is fully scaffolded but not yet wired into
// the server router (see server/handlers.rs — `notification_router` is
// intentionally unused on master). Allow dead_code at the module level so
// clippy doesn't fight the staged rollout; the wire-up lands in a follow-up.
#![allow(dead_code)]

//! Multi-channel alert notification engine.
//!
//! When a threat or monitor alert fires, this engine fans out notifications to
//! all registered channels: Webhook, Email (SMTP), Slack, and Telegram.
//!
//! # Architecture
//! - `NotificationEngine` owns registered `NotificationChannel`s
//! - A background task subscribes to the alert broadcast channel and calls `fan_out`
//! - Each send attempt is retried up to 3× with exponential backoff (1s, 2s, 4s)
//!   plus ±25% cryptographically-seeded jitter so a downstream outage doesn't
//!   produce a thundering herd from N alerts × M channels.
//! - SSRF defence — every outbound HTTP target (Webhook, Slack, Telegram) is
//!   sent through `orp_security::url_safety::build_safe_client`, the same
//!   validate-then-pin primitive used by the HTTP poller. Operators can opt
//!   into private/loopback targets per-channel via `allow_private_targets`.
//! - Per-channel circuit breaker — after `circuit_breaker_threshold`
//!   consecutive failures (default 5) we mark the channel "broken" for
//!   `circuit_breaker_cooldown` (default 5 min). During cooldown sends are
//!   short-circuited with a single warn log + audit entry rather than
//!   pounding on a dead endpoint.
//! - All attempts are logged to a bounded in-memory audit log
//!   (`MAX_AUDIT_ENTRIES = 10_000`, oldest dropped on overflow).

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get, post},
    Router,
};
use chrono::{DateTime, Utc};
use orp_security::url_safety::build_safe_client;
use rand::rngs::OsRng;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, Mutex, RwLock};
use tracing::{error, info, warn};
use uuid::Uuid;

// ── Channel types ─────────────────────────────────────────────────────────────

/// The type and configuration of a notification channel.
///
/// Every HTTP-based variant carries an `allow_private_targets` flag which,
/// when `true`, opts the channel out of the SSRF guard so it can deliver to
/// loopback / RFC1918 addresses. Default is `false` — without this default,
/// any user able to register a channel could pivot ORP into the cloud
/// metadata service or a co-located internal API. See
/// `orp_security::url_safety` for the underlying primitive.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChannelConfig {
    /// HTTP POST with JSON alert payload; optional shared secret in `X-ORP-Secret` header.
    Webhook {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        secret: Option<String>,
        /// Opt out of the SSRF guard for this channel — required for legitimate
        /// localhost integrations (e.g. an in-cluster sidecar).
        #[serde(default)]
        allow_private_targets: bool,
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
    Slack {
        webhook_url: String,
        #[serde(default)]
        allow_private_targets: bool,
    },
    /// Telegram Bot API — send_message.
    Telegram {
        bot_token: String,
        chat_id: String,
        #[serde(default)]
        allow_private_targets: bool,
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
    /// Number of consecutive send failures before the per-channel circuit
    /// breaker trips and skips the channel for `circuit_breaker_cooldown`.
    #[serde(default = "default_circuit_threshold")]
    pub circuit_breaker_threshold: u32,
    /// Cooldown duration (seconds) once the breaker is open. During cooldown
    /// fan-out logs a warning and skips the channel rather than retrying.
    #[serde(default = "default_circuit_cooldown_secs")]
    pub circuit_breaker_cooldown_secs: u64,
}

/// Request body for registering a new channel.
#[derive(Debug, Deserialize)]
pub struct RegisterChannelRequest {
    pub name: String,
    pub config: ChannelConfig,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_circuit_threshold")]
    pub circuit_breaker_threshold: u32,
    #[serde(default = "default_circuit_cooldown_secs")]
    pub circuit_breaker_cooldown_secs: u64,
}

fn default_true() -> bool {
    true
}

fn default_circuit_threshold() -> u32 {
    5
}

fn default_circuit_cooldown_secs() -> u64 {
    300 // 5 minutes
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

/// Maximum number of attempts retained in memory. Oldest entries are dropped
/// once the cap is hit. At ~250 bytes per entry this caps the audit log at
/// roughly ~2.5MB regardless of uptime.
pub const MAX_AUDIT_ENTRIES: usize = 10_000;

/// Bounded in-memory audit log for notification attempts.
///
/// `entries` is a `VecDeque` capped at [`MAX_AUDIT_ENTRIES`]; once full,
/// `record` pops the oldest entry before pushing the new one. A single warn
/// log is emitted at most once per minute while overflowing — the log is
/// best-effort observability and we don't want to flood it at steady-state
/// overflow.
pub struct NotificationAuditLog {
    entries: VecDeque<NotificationAttempt>,
    /// Last time we emitted the "audit log is overflowing" warning.
    /// `Instant` rather than `DateTime` so this is monotonic and immune to
    /// wall-clock skew.
    last_overflow_warn: Option<Instant>,
}

impl Default for NotificationAuditLog {
    fn default() -> Self {
        Self {
            entries: VecDeque::with_capacity(MAX_AUDIT_ENTRIES),
            last_overflow_warn: None,
        }
    }
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
        if self.entries.len() >= MAX_AUDIT_ENTRIES {
            self.entries.pop_front();
            // Rate-limit the overflow warning to once per minute so a steady
            // stream of alerts at the cap doesn't drown the log.
            let should_warn = self
                .last_overflow_warn
                .is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(60));
            if should_warn {
                warn!(
                    cap = MAX_AUDIT_ENTRIES,
                    "notification audit log at cap — dropping oldest entries"
                );
                self.last_overflow_warn = Some(Instant::now());
            }
        }
        self.entries.push_back(entry);
    }

    /// Borrow all entries currently retained (oldest → newest).
    pub fn all(&self) -> Vec<NotificationAttempt> {
        self.entries.iter().cloned().collect()
    }

    /// Most recent `n` entries (newest → oldest), capped at [`MAX_AUDIT_ENTRIES`].
    pub fn recent(&self, n: usize) -> Vec<NotificationAttempt> {
        let take = n.min(MAX_AUDIT_ENTRIES);
        self.entries.iter().rev().take(take).cloned().collect()
    }

    /// Current entry count (≤ [`MAX_AUDIT_ENTRIES`]).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the log holds zero entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ── Circuit breaker ───────────────────────────────────────────────────────────

/// Per-channel breaker state. `Instant` is monotonic so cooldown checks are
/// immune to wall-clock skew.
#[derive(Debug, Clone, Default)]
struct CircuitState {
    consecutive_failures: u32,
    /// Set once the breaker trips. Sends are short-circuited until `Instant::now() >= open_until`.
    open_until: Option<Instant>,
}

// ── Engine ────────────────────────────────────────────────────────────────────

/// Multi-channel notification engine.
///
/// Clone-cheap: internally `Arc`-backed.
#[derive(Clone)]
pub struct NotificationEngine {
    channels: Arc<RwLock<HashMap<String, NotificationChannel>>>,
    audit_log: Arc<Mutex<NotificationAuditLog>>,
    /// Per-channel breaker state, keyed by `channel_id`. Lives next to
    /// `channels` so removing a channel can also drop its breaker entry.
    circuit: Arc<Mutex<HashMap<String, CircuitState>>>,
}

impl NotificationEngine {
    pub fn new() -> Self {
        Self {
            channels: Arc::new(RwLock::new(HashMap::new())),
            audit_log: Arc::new(Mutex::new(NotificationAuditLog::default())),
            circuit: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    // ── Channel CRUD ──────────────────────────────────────────────────────────

    pub async fn register_channel(
        &self,
        name: String,
        config: ChannelConfig,
        enabled: bool,
    ) -> NotificationChannel {
        self.register_channel_full(
            name,
            config,
            enabled,
            default_circuit_threshold(),
            default_circuit_cooldown_secs(),
        )
        .await
    }

    /// Register a channel with explicit circuit-breaker tuning. The convenience
    /// `register_channel` wraps this with the defaults (5 failures / 5 min).
    pub async fn register_channel_full(
        &self,
        name: String,
        config: ChannelConfig,
        enabled: bool,
        circuit_breaker_threshold: u32,
        circuit_breaker_cooldown_secs: u64,
    ) -> NotificationChannel {
        let ch = NotificationChannel {
            channel_id: Uuid::new_v4().to_string(),
            name,
            config,
            enabled,
            created_at: Utc::now(),
            circuit_breaker_threshold,
            circuit_breaker_cooldown_secs,
        };
        self.channels
            .write()
            .await
            .insert(ch.channel_id.clone(), ch.clone());
        // Initialise breaker state — done eagerly so concurrent fan-outs see
        // a consistent zero-failure starting point.
        self.circuit
            .lock()
            .await
            .insert(ch.channel_id.clone(), CircuitState::default());
        ch
    }

    pub async fn list_channels(&self) -> Vec<NotificationChannel> {
        let map = self.channels.read().await;
        let mut v: Vec<_> = map.values().cloned().collect();
        v.sort_by_key(|x| x.created_at);
        v
    }

    pub async fn get_channel(&self, id: &str) -> Option<NotificationChannel> {
        self.channels.read().await.get(id).cloned()
    }

    pub async fn remove_channel(&self, id: &str) -> bool {
        let removed = self.channels.write().await.remove(id).is_some();
        if removed {
            // Drop the breaker entry too — otherwise re-registering with the
            // same id (not currently possible, but defensive) would inherit
            // stale failure counts from a long-dead channel.
            self.circuit.lock().await.remove(id);
        }
        removed
    }

    // ── Fan-out ───────────────────────────────────────────────────────────────

    /// Send `payload` to every enabled channel, retrying each up to 3 times.
    pub async fn fan_out(&self, payload: &AlertPayload) {
        let channels: Vec<NotificationChannel> = {
            self.channels
                .read()
                .await
                .values()
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

    /// Returns `Some(open_until)` if the channel breaker is currently open,
    /// otherwise `None`. Self-healing: if `Instant::now() >= open_until` we
    /// transparently close the breaker (zero out the failure counter) so
    /// the next call attempts a real send.
    async fn breaker_check(&self, channel_id: &str) -> Option<Instant> {
        let mut map = self.circuit.lock().await;
        let state = map.entry(channel_id.to_string()).or_default();
        if let Some(until) = state.open_until {
            if Instant::now() < until {
                return Some(until);
            }
            // Cooldown has elapsed — close the breaker.
            state.open_until = None;
            state.consecutive_failures = 0;
        }
        None
    }

    /// Record one success. Resets the consecutive-failure counter and ensures
    /// the breaker is closed.
    async fn breaker_record_success(&self, channel_id: &str) {
        let mut map = self.circuit.lock().await;
        let state = map.entry(channel_id.to_string()).or_default();
        state.consecutive_failures = 0;
        state.open_until = None;
    }

    /// Record one failure. Returns `true` if this failure tripped the breaker.
    async fn breaker_record_failure(&self, channel: &NotificationChannel) -> bool {
        let mut map = self.circuit.lock().await;
        let state = map.entry(channel.channel_id.clone()).or_default();
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        if state.open_until.is_none()
            && state.consecutive_failures >= channel.circuit_breaker_threshold
        {
            state.open_until = Some(
                Instant::now()
                    + std::time::Duration::from_secs(channel.circuit_breaker_cooldown_secs),
            );
            return true;
        }
        false
    }

    /// Compute the retry sleep for `attempt` (1-indexed): exponential
    /// `2^(attempt-1)` seconds × cryptographically-seeded ±25% jitter.
    ///
    /// `OsRng` (not `thread_rng`) is required: this affects security-relevant
    /// timing — knowing the exact retry schedule lets an attacker line up
    /// failure windows on a downstream they control. Cryptographic randomness
    /// closes that side channel.
    fn jittered_backoff(attempt: u32) -> std::time::Duration {
        let base_secs = 1u64 << (attempt - 1) as u64; // 1, 2, 4, …
                                                      // gen_range on f64 is half-open: 0.75..=1.25 maps to a closed range so
                                                      // we hit both bounds.
        let factor: f64 = OsRng.gen_range(0.75..=1.25);
        let secs = (base_secs as f64) * factor;
        std::time::Duration::from_millis((secs * 1000.0) as u64)
    }

    /// Attempt to send to a single channel, retrying up to 3× with exponential
    /// backoff + jitter. Honours the per-channel circuit breaker — when open,
    /// the call short-circuits with a single warn log + audit entry.
    async fn send_with_retry(&self, channel: &NotificationChannel, payload: &AlertPayload) {
        const MAX_ATTEMPTS: u32 = 3;

        // Circuit-breaker fast path. If the breaker is open we don't even
        // probe the downstream — that's the whole point of the breaker.
        if let Some(open_until) = self.breaker_check(&channel.channel_id).await {
            let secs_remaining = open_until
                .saturating_duration_since(Instant::now())
                .as_secs();
            warn!(
                event = "notification_circuit_broken",
                channel = %channel.channel_id,
                channel_type = %channel.config.type_label(),
                cooldown_remaining_secs = secs_remaining,
                "skipping notification — circuit breaker open"
            );
            self.audit_log.lock().await.record(NotificationAttempt {
                attempt_id: Uuid::new_v4().to_string(),
                channel_id: channel.channel_id.clone(),
                channel_type: channel.config.type_label().to_string(),
                alert_id: payload.alert_id.clone(),
                outcome: AttemptOutcome::Failure,
                error: Some(format!(
                    "circuit_broken: cooldown {secs_remaining}s remaining"
                )),
                attempts: 0,
                timestamp: Utc::now(),
            });
            return;
        }

        let mut last_error: Option<String> = None;

        for attempt in 1..=MAX_ATTEMPTS {
            match self.send_once(channel, payload).await {
                Ok(()) => {
                    self.breaker_record_success(&channel.channel_id).await;
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
                        tokio::time::sleep(Self::jittered_backoff(attempt)).await;
                    }
                }
            }
        }

        // All attempts exhausted — record breaker bookkeeping and audit.
        let tripped = self.breaker_record_failure(channel).await;
        error!(
            channel_id = %channel.channel_id,
            alert_id = %payload.alert_id,
            tripped_breaker = tripped,
            "notification delivery failed after {} attempts", MAX_ATTEMPTS
        );
        if tripped {
            warn!(
                event = "notification_circuit_broken",
                channel = %channel.channel_id,
                channel_type = %channel.config.type_label(),
                threshold = channel.circuit_breaker_threshold,
                cooldown_secs = channel.circuit_breaker_cooldown_secs,
                "circuit breaker tripped"
            );
        }
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
    async fn send_once(
        &self,
        channel: &NotificationChannel,
        payload: &AlertPayload,
    ) -> Result<(), String> {
        match &channel.config {
            ChannelConfig::Webhook {
                url,
                secret,
                allow_private_targets,
            } => {
                self.send_webhook(url, secret.as_deref(), *allow_private_targets, payload)
                    .await
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
                    smtp_host,
                    *smtp_port,
                    username,
                    password,
                    from_address,
                    to_addresses,
                    payload,
                )
                .await
            }
            ChannelConfig::Slack {
                webhook_url,
                allow_private_targets,
            } => {
                self.send_slack(webhook_url, *allow_private_targets, payload)
                    .await
            }
            ChannelConfig::Telegram {
                bot_token,
                chat_id,
                allow_private_targets,
            } => {
                self.send_telegram(bot_token, chat_id, *allow_private_targets, payload)
                    .await
            }
        }
    }

    // ── Webhook ───────────────────────────────────────────────────────────────

    async fn send_webhook(
        &self,
        url: &str,
        secret: Option<&str>,
        allow_private: bool,
        payload: &AlertPayload,
    ) -> Result<(), String> {
        // Validate-then-pin: same defence the HTTP poller uses. Without this
        // a webhook channel pointed at `http://169.254.169.254/...` would
        // happily exfiltrate cloud-provider creds to anyone who can register
        // a notification channel.
        let (client, _addrs) = build_safe_client(url, allow_private).map_err(|reason| {
            warn!(
                url = %url,
                reason = %reason,
                "SSRF guard blocked webhook channel"
            );
            format!("SSRF guard blocked webhook target: {reason}")
        })?;
        let mut req = client.post(url).json(payload);
        if let Some(s) = secret {
            req = req.header("X-ORP-Secret", s);
        }
        req = req.header("Content-Type", "application/json");
        let resp = req
            .send()
            .await
            .map_err(|e| format!("webhook send error: {e}"))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("webhook returned HTTP {}", resp.status()))
        }
    }

    // ── Slack ─────────────────────────────────────────────────────────────────

    async fn send_slack(
        &self,
        webhook_url: &str,
        allow_private: bool,
        payload: &AlertPayload,
    ) -> Result<(), String> {
        // Slack URLs are well-known (`hooks.slack.com`) but we still run them
        // through `build_safe_client` so a misconfigured channel pointed at
        // an internal proxy can't bypass SSRF defence by claiming "Slack".
        let (client, _addrs) = build_safe_client(webhook_url, allow_private).map_err(|reason| {
            warn!(
                url = %webhook_url,
                reason = %reason,
                "SSRF guard blocked slack channel"
            );
            format!("SSRF guard blocked slack target: {reason}")
        })?;
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
        let resp = client
            .post(webhook_url)
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
        allow_private: bool,
        payload: &AlertPayload,
    ) -> Result<(), String> {
        let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);
        // Telegram targets api.telegram.org but we still validate to defend
        // against operator misconfiguration / DNS-rebinding-style attacks
        // against that hostname.
        let (client, _addrs) = build_safe_client(&url, allow_private).map_err(|reason| {
            warn!(
                url = %url,
                reason = %reason,
                "SSRF guard blocked telegram channel"
            );
            format!("SSRF guard blocked telegram target: {reason}")
        })?;
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
        let body = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": "Markdown"
        });
        let resp = client
            .post(&url)
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
        self.audit_log.lock().await.all()
    }

    /// Most recent `n` audit entries (newest → oldest), capped at
    /// [`MAX_AUDIT_ENTRIES`]. Suitable for a `/diagnostics` endpoint.
    pub async fn recent_audit(&self, n: usize) -> Vec<NotificationAttempt> {
        self.audit_log.lock().await.recent(n)
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
            reader
                .read_line(&mut line)
                .await
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
    writer
        .write_all(b"EHLO orp-notifications\r\n")
        .await
        .map_err(|e| format!("SMTP EHLO write: {e}"))?;
    // Read all EHLO response lines (multi-line: 250-)
    loop {
        let line = smtp_read!("EHLO");
        if line.starts_with("250 ") || line.starts_with("250\r") || line.starts_with("250\n") {
            break;
        }
        if !line.starts_with("250-") {
            return Err(format!("SMTP EHLO unexpected: {}", line.trim()));
        }
    }

    // AUTH LOGIN
    writer
        .write_all(b"AUTH LOGIN\r\n")
        .await
        .map_err(|e| format!("SMTP AUTH write: {e}"))?;
    let auth_prompt = smtp_read!("AUTH prompt");
    if !auth_prompt.starts_with("334") {
        return Err(format!(
            "SMTP AUTH LOGIN not accepted: {}",
            auth_prompt.trim()
        ));
    }
    writer
        .write_all(format!("{}\r\n", B64.encode(username)).as_bytes())
        .await
        .map_err(|e| format!("SMTP write username: {e}"))?;
    let user_resp = smtp_read!("username resp");
    if !user_resp.starts_with("334") {
        return Err(format!("SMTP username not accepted: {}", user_resp.trim()));
    }
    writer
        .write_all(format!("{}\r\n", B64.encode(password)).as_bytes())
        .await
        .map_err(|e| format!("SMTP write password: {e}"))?;
    let pass_resp = smtp_read!("password resp");
    if !pass_resp.starts_with("235") {
        return Err(format!("SMTP AUTH failed: {}", pass_resp.trim()));
    }

    // MAIL FROM
    writer
        .write_all(format!("MAIL FROM:<{}>\r\n", from).as_bytes())
        .await
        .map_err(|e| format!("SMTP MAIL FROM write: {e}"))?;
    let mail_resp = smtp_read!("MAIL FROM resp");
    if !mail_resp.starts_with("250") {
        return Err(format!("SMTP MAIL FROM rejected: {}", mail_resp.trim()));
    }

    // RCPT TO
    for rcpt_addr in to_addresses {
        writer
            .write_all(format!("RCPT TO:<{}>\r\n", rcpt_addr).as_bytes())
            .await
            .map_err(|e| format!("SMTP RCPT TO write: {e}"))?;
        let rcpt_resp = smtp_read!("RCPT TO resp");
        if !rcpt_resp.starts_with("250") && !rcpt_resp.starts_with("251") {
            return Err(format!(
                "SMTP RCPT TO rejected for {rcpt_addr}: {}",
                rcpt_resp.trim()
            ));
        }
    }

    // DATA
    writer
        .write_all(b"DATA\r\n")
        .await
        .map_err(|e| format!("SMTP DATA cmd: {e}"))?;
    let data_resp = smtp_read!("DATA resp");
    if !data_resp.starts_with("354") {
        return Err(format!("SMTP DATA not accepted: {}", data_resp.trim()));
    }
    writer
        .write_all(format!("{}\r\n.\r\n", mime).as_bytes())
        .await
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
    let ch = state
        .engine
        .register_channel_full(
            req.name,
            req.config,
            req.enabled,
            req.circuit_breaker_threshold,
            req.circuit_breaker_cooldown_secs,
        )
        .await;
    info!(channel_id = %ch.channel_id, channel_type = %ch.config.type_label(), "registered notification channel");
    Ok((StatusCode::CREATED, Json(ch)))
}

/// List `GET /api/v1/notifications/channels`
async fn list_channels(State(state): State<NotificationState>) -> Json<Vec<NotificationChannel>> {
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
    let last = entries
        .iter()
        .rev()
        .find(|e| e.channel_id == id && e.alert_id == test_payload.alert_id);
    let outcome = last
        .map(|e| e.outcome.clone())
        .unwrap_or(AttemptOutcome::Failure);

    if outcome == AttemptOutcome::Success {
        Ok(Json(serde_json::json!({
            "status": "ok",
            "channel_id": id,
            "alert_id": test_payload.alert_id
        })))
    } else {
        let err_msg = last
            .and_then(|e| e.error.as_deref())
            .unwrap_or("unknown error");
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
        let ch = eng
            .register_channel(
                "test-webhook".into(),
                ChannelConfig::Webhook {
                    url: "http://localhost:9999".into(),
                    secret: None,
                    allow_private_targets: true,
                },
                true,
            )
            .await;
        assert!(!ch.channel_id.is_empty());
        assert_eq!(ch.name, "test-webhook");

        let list = eng.list_channels().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].channel_id, ch.channel_id);
    }

    #[tokio::test]
    async fn test_remove_channel_returns_true() {
        let eng = engine();
        let ch = eng
            .register_channel(
                "slack".into(),
                ChannelConfig::Slack {
                    webhook_url: "https://hooks.slack.com/x".into(),
                    allow_private_targets: false,
                },
                true,
            )
            .await;
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
        let ch = eng
            .register_channel(
                "tg".into(),
                ChannelConfig::Telegram {
                    bot_token: "token".into(),
                    chat_id: "123".into(),
                    allow_private_targets: false,
                },
                true,
            )
            .await;
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
            ChannelConfig::Webhook {
                url: "http://localhost:19999/nope".into(),
                secret: None,
                allow_private_targets: true,
            },
            false,
        )
        .await;

        let payload = make_payload("WARNING");
        // fan_out should not attempt to send (no active channel)
        eng.fan_out(&payload).await;
        // Give any spurious spawns a moment
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let entries = eng.audit_entries().await;
        assert!(
            entries.is_empty(),
            "disabled channel should not have been attempted"
        );
    }

    // ── channel type labels ───────────────────────────────────────────────────

    #[test]
    fn test_channel_type_labels() {
        assert_eq!(
            ChannelConfig::Webhook {
                url: "x".into(),
                secret: None,
                allow_private_targets: false,
            }
            .type_label(),
            "webhook"
        );
        assert_eq!(
            ChannelConfig::Slack {
                webhook_url: "x".into(),
                allow_private_targets: false,
            }
            .type_label(),
            "slack"
        );
        assert_eq!(
            ChannelConfig::Telegram {
                bot_token: "t".into(),
                chat_id: "c".into(),
                allow_private_targets: false,
            }
            .type_label(),
            "telegram"
        );
        assert_eq!(
            ChannelConfig::Email {
                smtp_host: "h".into(),
                smtp_port: 587,
                username: "u".into(),
                password: "p".into(),
                from_address: "f@e.com".into(),
                to_addresses: vec![],
            }
            .type_label(),
            "email"
        );
    }

    // ── audit log ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_audit_log_records_failure() {
        let eng = engine();
        // Disable circuit breaker for this test by setting threshold extremely
        // high — we want the retry path to exhaust all 3 attempts and the
        // breaker not to short-circuit the 3rd.
        let ch = eng
            .register_channel_full(
                "bad-webhook".into(),
                // Port 1 is refused on every OS. allow_private_targets=true so
                // we test connect-refused (the actual retry path) rather than
                // the SSRF guard which would fail-fast on the 1st attempt.
                ChannelConfig::Webhook {
                    url: "http://127.0.0.1:1/bad".into(),
                    secret: None,
                    allow_private_targets: true,
                },
                true,
                u32::MAX,
                300,
            )
            .await;
        let payload = make_payload("CRITICAL");
        // Override jittered backoff for test speed by using an empty
        // payload; the retry path still records 3 attempts.
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
        let ch = eng
            .register_channel(
                "tg-bad".into(),
                ChannelConfig::Telegram {
                    bot_token: "invalid_token".into(),
                    chat_id: "0".into(),
                    allow_private_targets: false,
                },
                true,
            )
            .await;
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
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
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
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
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

    // ── SSRF guard ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_ssrf_guard_blocks_loopback_webhook_by_default() {
        // No `allow_private_targets` → must fail fast with the SSRF reason
        // from the very first attempt and never even open a TCP socket.
        let eng = engine();
        let ch = eng
            .register_channel(
                "ssrf-test".into(),
                ChannelConfig::Webhook {
                    url: "http://127.0.0.1:9/should-be-blocked".into(),
                    secret: None,
                    allow_private_targets: false,
                },
                true,
            )
            .await;
        let payload = make_payload("CRITICAL");
        eng.send_with_retry(&ch, &payload).await;

        let entries = eng.audit_entries().await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].outcome, AttemptOutcome::Failure);
        // Each of the 3 attempts hits the same SSRF guard error → MAX_ATTEMPTS.
        assert_eq!(entries[0].attempts, 3);
        let err = entries[0].error.as_deref().unwrap_or_default();
        assert!(
            err.contains("SSRF guard"),
            "expected SSRF guard reject, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_ssrf_guard_blocks_cloud_metadata_slack() {
        let eng = engine();
        let ch = eng
            .register_channel(
                "metadata-slack".into(),
                ChannelConfig::Slack {
                    webhook_url: "http://169.254.169.254/latest/meta-data/".into(),
                    allow_private_targets: false,
                },
                true,
            )
            .await;
        let payload = make_payload("WARNING");
        eng.send_with_retry(&ch, &payload).await;

        let entries = eng.audit_entries().await;
        assert!(entries.iter().any(|e| e.outcome == AttemptOutcome::Failure
            && e.error.as_deref().unwrap_or("").contains("SSRF guard")));
    }

    #[tokio::test]
    async fn test_ssrf_guard_allows_when_opted_in() {
        // With allow_private_targets=true the SSRF guard is bypassed and we
        // get the underlying connect error (or HTTP error) instead — proving
        // the guard is the only thing blocking us.
        let eng = engine();
        let ch = eng
            .register_channel_full(
                "ssrf-opt-in".into(),
                ChannelConfig::Webhook {
                    // Port 1 is reliably refused on every OS.
                    url: "http://127.0.0.1:1/".into(),
                    secret: None,
                    allow_private_targets: true,
                },
                true,
                u32::MAX,
                300,
            )
            .await;
        let payload = make_payload("INFO");
        eng.send_with_retry(&ch, &payload).await;

        let entries = eng.audit_entries().await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].outcome, AttemptOutcome::Failure);
        let err = entries[0].error.as_deref().unwrap_or_default();
        // We want a connect error here, NOT the SSRF rejection.
        assert!(
            !err.contains("SSRF guard"),
            "allow_private_targets=true should bypass SSRF guard, got: {err}"
        );
    }

    // ── Retry jitter ──────────────────────────────────────────────────────────

    #[test]
    fn test_jittered_backoff_within_bounds() {
        // attempt=1 → base=1s; jitter 0.75..=1.25 → 750..=1250 ms
        // attempt=2 → base=2s → 1500..=2500 ms
        // attempt=3 → base=4s → 3000..=5000 ms
        // Drawing 200 samples per attempt and asserting all stay in band.
        for (attempt, lo_ms, hi_ms) in [(1u32, 750u128, 1250u128), (2, 1500, 2500), (3, 3000, 5000)]
        {
            for _ in 0..200 {
                let d = NotificationEngine::jittered_backoff(attempt).as_millis();
                assert!(
                    d >= lo_ms && d <= hi_ms,
                    "attempt {attempt}: {d}ms out of [{lo_ms},{hi_ms}]"
                );
            }
        }
    }

    #[test]
    fn test_jittered_backoff_actually_jitters() {
        // 50 draws — at least 5 distinct values. If we ever accidentally
        // wired in a constant the test fails loudly.
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50 {
            seen.insert(NotificationEngine::jittered_backoff(2).as_millis());
        }
        assert!(
            seen.len() >= 5,
            "expected jitter, got {} unique",
            seen.len()
        );
    }

    // ── Circuit breaker ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_circuit_opens_after_threshold_failures() {
        let eng = engine();
        // 2-fail threshold + 60s cooldown so the test trips the breaker on
        // the 2nd consecutive failure and short-circuits the 3rd attempt.
        let ch = eng
            .register_channel_full(
                "flaky".into(),
                // SSRF guard rejects every attempt cleanly + fast — the
                // perfect deterministic failure source for a breaker test.
                ChannelConfig::Webhook {
                    url: "http://127.0.0.1:1/blocked".into(),
                    secret: None,
                    allow_private_targets: false,
                },
                true,
                2,
                60,
            )
            .await;
        let payload = make_payload("CRITICAL");

        // First two send_with_retry calls each exhaust 3 attempts and trip
        // the breaker on the 2nd. The third call should short-circuit.
        eng.send_with_retry(&ch, &payload).await;
        eng.send_with_retry(&ch, &payload).await;
        eng.send_with_retry(&ch, &payload).await;

        let entries = eng.audit_entries().await;
        assert_eq!(entries.len(), 3, "expected 3 audit entries");
        // 1st & 2nd: 3 attempts each (exhausted retries).
        assert_eq!(entries[0].attempts, 3);
        assert_eq!(entries[1].attempts, 3);
        // 3rd: 0 attempts — circuit was open.
        assert_eq!(entries[2].attempts, 0);
        let err3 = entries[2].error.as_deref().unwrap_or_default();
        assert!(
            err3.contains("circuit_broken"),
            "expected circuit_broken short-circuit, got: {err3}"
        );
    }

    #[tokio::test]
    async fn test_circuit_closes_after_cooldown() {
        // Trip with a 1-fail threshold, then wait past a 1-second cooldown —
        // the next attempt should NOT short-circuit (it should make the call
        // and fail with the SSRF error again, but `attempts > 0`).
        let eng = engine();
        let ch = eng
            .register_channel_full(
                "cooldown-test".into(),
                ChannelConfig::Webhook {
                    url: "http://127.0.0.1:1/blocked".into(),
                    secret: None,
                    allow_private_targets: false,
                },
                true,
                1,
                1, // 1s cooldown
            )
            .await;
        let payload = make_payload("INFO");
        eng.send_with_retry(&ch, &payload).await; // trips after 1st batch
                                                  // While breaker is open: short-circuit (attempts==0).
        eng.send_with_retry(&ch, &payload).await;

        // Wait out the cooldown — 1.2s gives a small safety margin.
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

        // Now the breaker must be closed; this call should run all 3 attempts.
        eng.send_with_retry(&ch, &payload).await;

        let entries = eng.audit_entries().await;
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].attempts, 3);
        assert_eq!(entries[1].attempts, 0); // short-circuited
        assert_eq!(entries[2].attempts, 3); // closed → fully retried again
    }

    // ── Bounded audit log ─────────────────────────────────────────────────────

    #[test]
    fn test_audit_log_caps_at_max() {
        let mut log = NotificationAuditLog::default();
        for i in 0..(MAX_AUDIT_ENTRIES + 1) {
            log.record(NotificationAttempt {
                attempt_id: format!("a{i}"),
                channel_id: "c".into(),
                channel_type: "webhook".into(),
                alert_id: format!("alert-{i}"),
                outcome: AttemptOutcome::Success,
                error: None,
                attempts: 1,
                timestamp: Utc::now(),
            });
        }
        assert_eq!(log.len(), MAX_AUDIT_ENTRIES);
        // Oldest (i=0, alert-0) must have been dropped.
        let all = log.all();
        assert!(
            !all.iter().any(|e| e.alert_id == "alert-0"),
            "oldest entry should have been popped"
        );
        // Newest must still be there.
        assert_eq!(
            all.last().map(|e| e.alert_id.as_str()),
            Some(format!("alert-{}", MAX_AUDIT_ENTRIES).as_str())
        );
    }

    #[test]
    fn test_audit_log_recent_returns_newest_first() {
        let mut log = NotificationAuditLog::default();
        for i in 0..10 {
            log.record(NotificationAttempt {
                attempt_id: format!("a{i}"),
                channel_id: "c".into(),
                channel_type: "webhook".into(),
                alert_id: format!("alert-{i}"),
                outcome: AttemptOutcome::Success,
                error: None,
                attempts: 1,
                timestamp: Utc::now(),
            });
        }
        let recent = log.recent(3);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].alert_id, "alert-9");
        assert_eq!(recent[1].alert_id, "alert-8");
        assert_eq!(recent[2].alert_id, "alert-7");
        // recent(n) is capped at MAX_AUDIT_ENTRIES even if n is larger.
        let huge = log.recent(usize::MAX);
        assert_eq!(huge.len(), 10);
    }
}
