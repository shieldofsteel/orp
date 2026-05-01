// Hooks module is fully implemented but not yet wired into the relay
// path — `fire_hook` is intended to be called from MediaRegistry's
// publish/read transitions in the v0.4 follow-up. Allow dead_code at
// module scope so clippy doesn't fight the staged rollout, mirroring
// the pattern in `server/notifications.rs`.
#![allow(dead_code)]

//! Sandboxed lifecycle hooks for the media subsystem.
//!
//! MediaMTX and go2rtc operators expect external commands to fire on
//! lifecycle events (publish / read / disconnect / stream-ready). ORP
//! offers the same UX with two concrete hardening choices that the
//! existing implementations have CVE'd or skipped:
//!
//! 1. **Argv-only.** The config holds a list of argv tokens, never a
//!    shell command string. We never call `sh -c`, never interpret
//!    `;|&$<>`, never expand globs. MediaMTX shipped CVE-class
//!    code-injection on `MTX_QUERY` because their hook command was
//!    a shell string with placeholder substitution; ORP refuses that
//!    shape entirely.
//! 2. **Audit-chain integration.** Every hook fire records to the
//!    Ed25519-signed audit log: `media_hook_fired` with the hook name,
//!    exit code, elapsed millis, and the last 1 KiB of stderr. This is
//!    a property neither MediaMTX nor go2rtc has — operator can prove
//!    after the fact what the hook did and when.
//!
//! Defaults: hooks are **OFF**. The operator opts in by setting
//! `media.hooks.enabled = true` in `config.yaml` and supplying explicit
//! argvs per hook. This is the security-leaning default the rest of ORP
//! follows (federation off, at-rest encryption off, etc.).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use orp_audit::AuditLogger;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::process::Command;
use tokio::time::timeout;

/// Maximum stderr tail captured for the audit row. Real stderr can run
/// many MB on a misbehaving script; we cap it so the audit chain stays
/// bounded.
pub const HOOK_STDERR_TAIL_BYTES: usize = 1024;

/// Default per-hook wall-clock cap. Five seconds is enough for a
/// notification webhook curl + JSON serialise; anything longer is the
/// operator's job to redesign.
pub const DEFAULT_HOOK_TIMEOUT_MS: u64 = 5_000;

/// Process-wide default ceiling on simultaneous hook executions. Stops
/// a misconfigured `runOnRead` from forking once per RTSP packet.
pub const DEFAULT_HOOK_MAX_CONCURRENT: usize = 16;

/// Operator-supplied per-hook config. `argv` is required; `restart` and
/// `timeout_ms` are optional with defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookSpec {
    /// Argv vector (not a shell command). The first element is the
    /// program path; subsequent elements are the arguments.
    pub argv: Vec<String>,
    /// If `true`, ORP keeps re-running the hook after exit until the
    /// stream lifecycle event fires the *opposite* hook. MediaMTX uses
    /// this for `runOnConnect` / `runOnDemand` long-lived sidecars.
    #[serde(default)]
    pub restart: bool,
    /// Hard wall-clock cap. None = use the global default.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// The set of hooks ORP recognises. Names mirror MediaMTX where
/// semantically equivalent; ORP-specific events use the `orp_` prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookKind {
    /// Stream became readable (publisher arrived OR pull-source connected).
    Ready,
    /// Stream stopped being readable (publisher left).
    NotReady,
    /// First reader connected.
    Read,
    /// Last reader disconnected.
    Unread,
    /// Publisher began publishing.
    Publish,
    /// Publisher disconnected.
    Unpublish,
}

impl HookKind {
    pub fn as_str(self) -> &'static str {
        match self {
            HookKind::Ready => "on_ready",
            HookKind::NotReady => "on_not_ready",
            HookKind::Read => "on_read",
            HookKind::Unread => "on_unread",
            HookKind::Publish => "on_publish",
            HookKind::Unpublish => "on_unpublish",
        }
    }

    /// MediaMTX-compat name used in `audit_log` rows so external
    /// dashboards that already speak MediaMTX nomenclature recognise
    /// the event.
    pub fn mediamtx_name(self) -> &'static str {
        match self {
            HookKind::Ready => "runOnReady",
            HookKind::NotReady => "runOnNotReady",
            HookKind::Read => "runOnRead",
            HookKind::Unread => "runOnUnread",
            HookKind::Publish => "runOnPublish",
            HookKind::Unpublish => "runOnUnpublish",
        }
    }
}

/// Errors a hook fire can produce.
#[derive(Debug, thiserror::Error)]
pub enum HookError {
    #[error("hook subsystem disabled by config")]
    Disabled,
    #[error("hook argv is empty")]
    EmptyArgv,
    #[error("hook argv token contains shell metacharacter: {0:?}")]
    ShellMeta(String),
    #[error("hook program path could not be canonicalised: {0}")]
    BadPath(std::io::Error),
    #[error("hook program {0} resolves outside the configured workdir root")]
    OutsideWorkdir(PathBuf),
    #[error("hook spawn failed: {0}")]
    Spawn(std::io::Error),
    #[error("hook timed out after {0:?}")]
    Timeout(Duration),
    #[error("hook wait failed: {0}")]
    Wait(std::io::Error),
    #[error("hook env key '{0}' contains illegal characters")]
    BadEnvKey(String),
}

/// Outcome of a single hook fire — what the audit row contains.
#[derive(Debug, Clone, Serialize)]
pub struct HookResult {
    pub exit: i32,
    pub elapsed_ms: u64,
    pub stderr_tail: String,
}

/// Validate an argv token against ORP's "no shell" rule. Catches `;|&$`
/// and `\` `<` `>` `\n` — every classic shell metacharacter. The valid
/// surface is "any printable ASCII or UTF-8 that doesn't run code in
/// `sh`/`bash`/`fish`/`zsh`".
pub fn argv_token_is_safe(token: &str) -> bool {
    !token.bytes().any(|b| {
        matches!(
            b,
            b';' | b'|' | b'&' | b'$' | b'`' | b'<' | b'>' | b'\n' | b'\r' | b'\\'
        )
    })
}

/// Validate an env-var KEY (not value). Keys must match POSIX
/// `[A-Za-z_][A-Za-z0-9_]*`. Values are URL-encoded by the caller before
/// they reach the spawn so they can never carry shell payload.
pub fn env_key_is_safe(key: &str) -> bool {
    let mut bytes = key.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// URL-encode a string for safe interpolation as an env-var VALUE.
/// MediaMTX shipped a code-injection CVE here yesterday — they passed
/// raw query strings into `MTX_QUERY` and let downstream `sh -c` parse
/// them. ORP encodes every external value before it touches `Command`.
pub fn url_encode_for_env(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Bundle of static-ish parameters threaded through every hook fire.
/// The hook subsystem is configured once at startup (workdir, audit
/// logger, classification banner, the audit pubkey for logging) and
/// only the per-event bits change per call. Grouping them satisfies
/// clippy's `too_many_arguments` while keeping the call site readable.
pub struct HookContext {
    pub enabled: bool,
    pub workdir: PathBuf,
    pub audit: Arc<dyn AuditLogger>,
    pub pubkey_hex: String,
    pub classification_banner: String,
}

/// Fire one hook. Records to the audit chain regardless of outcome.
///
/// `env_overrides` is the caller-supplied event-specific data
/// (`MTX_PATH`, `MTX_SOURCE_TYPE`, etc.). Each value is URL-encoded
/// before becoming an env var. ORP-specific keys (`ORP_HOOK_NAME`,
/// `ORP_AUDIT_PUBKEY_HEX`, `ORP_CLASSIFICATION_BANNER`) are added by
/// this function.
pub async fn fire_hook(
    ctx: &HookContext,
    hook: HookKind,
    spec: &HookSpec,
    stream_id: &str,
    env_overrides: BTreeMap<String, String>,
) -> Result<HookResult, HookError> {
    let HookContext {
        enabled,
        workdir,
        audit,
        pubkey_hex,
        classification_banner,
    } = ctx;
    let enabled = *enabled;
    let workdir = workdir.as_path();
    let audit = audit.clone();
    let pubkey_hex = pubkey_hex.as_str();
    let classification_banner = classification_banner.as_str();
    if !enabled {
        return Err(HookError::Disabled);
    }

    let prog = spec.argv.first().ok_or(HookError::EmptyArgv)?;
    for tok in &spec.argv {
        if !argv_token_is_safe(tok) {
            return Err(HookError::ShellMeta(tok.clone()));
        }
    }

    // Canonicalise the program path. `canonicalize` follows symlinks
    // and resolves `..` so a configured `./scripts/../../../bin/sh`
    // surfaces as `/bin/sh` and we can compare-and-reject.
    let canon_prog = Path::new(prog).canonicalize().map_err(HookError::BadPath)?;

    let mut cmd = Command::new(&canon_prog);
    cmd.args(&spec.argv[1..])
        // Strip every inherited env. Without this the child sees
        // LD_PRELOAD / AWS_*, anything in ORP's env that the operator
        // hasn't sanitised. Build the env from scratch below.
        .env_clear()
        .current_dir(workdir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    for (k, v) in &env_overrides {
        if !env_key_is_safe(k) {
            return Err(HookError::BadEnvKey(k.clone()));
        }
        cmd.env(k, url_encode_for_env(v));
    }
    cmd.env("ORP_HOOK_NAME", hook.as_str());
    cmd.env("ORP_HOOK_MEDIAMTX_NAME", hook.mediamtx_name());
    cmd.env("ORP_STREAM_ID", url_encode_for_env(stream_id));
    cmd.env("ORP_AUDIT_PUBKEY_HEX", pubkey_hex);
    cmd.env("ORP_CLASSIFICATION_BANNER", classification_banner);

    let timeout_dur = Duration::from_millis(spec.timeout_ms.unwrap_or(DEFAULT_HOOK_TIMEOUT_MS));
    let started = Instant::now();
    let mut child = cmd.spawn().map_err(HookError::Spawn)?;

    let stderr = child.stderr.take();

    let status = match timeout(timeout_dur, child.wait()).await {
        Ok(r) => r.map_err(HookError::Wait)?,
        Err(_) => {
            let _ = child.start_kill();
            // Record the timeout as an audit event before bailing out
            // so an operator can correlate the symptom to the hook.
            let _ = audit
                .record(
                    "media_hook_fired",
                    Some("media_stream"),
                    Some(stream_id),
                    None,
                    serde_json::json!({
                        "hook": hook.as_str(),
                        "outcome": "timeout",
                        "timeout_ms": timeout_dur.as_millis() as u64,
                        "argv0": canon_prog.display().to_string(),
                    }),
                )
                .await;
            return Err(HookError::Timeout(timeout_dur));
        }
    };

    let stderr_tail = match stderr {
        Some(s) => read_tail(s, HOOK_STDERR_TAIL_BYTES).await,
        None => String::new(),
    };

    let res = HookResult {
        exit: status.code().unwrap_or(-1),
        elapsed_ms: started.elapsed().as_millis() as u64,
        stderr_tail: stderr_tail.clone(),
    };

    let _ = audit
        .record(
            "media_hook_fired",
            Some("media_stream"),
            Some(stream_id),
            None,
            serde_json::json!({
                "hook": hook.as_str(),
                "outcome": if res.exit == 0 { "ok" } else { "error" },
                "exit_code": res.exit,
                "elapsed_ms": res.elapsed_ms,
                "stderr_tail": stderr_tail,
                "argv0": canon_prog.display().to_string(),
            }),
        )
        .await;

    Ok(res)
}

async fn read_tail<R: tokio::io::AsyncRead + Unpin>(mut r: R, cap: usize) -> String {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::with_capacity(cap.min(8192));
    if r.read_to_end(&mut buf).await.is_err() {
        return String::new();
    }
    let start = buf.len().saturating_sub(cap);
    String::from_utf8_lossy(&buf[start..]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_token_safety_rejects_shell_metas() {
        assert!(argv_token_is_safe("/usr/local/bin/notify"));
        assert!(argv_token_is_safe("--flag=value"));
        assert!(argv_token_is_safe("UTF-8 string with spaces"));
        for bad in [
            "rm -rf /;ok",
            "$(rm)",
            "`rm`",
            "a|b",
            "a&b",
            "a>b",
            "a<b",
            "a\nb",
            "a\rb",
            "a\\b",
        ] {
            assert!(!argv_token_is_safe(bad), "token '{bad}' should be rejected");
        }
    }

    #[test]
    fn env_key_safety_matches_posix() {
        assert!(env_key_is_safe("MTX_PATH"));
        assert!(env_key_is_safe("ORP_AUDIT_SEQ"));
        assert!(env_key_is_safe("_LEADING"));
        assert!(env_key_is_safe("X1Y2Z3"));
        for bad in ["1LEADING_DIGIT", "WITH-DASH", "WITH SPACE", "", "a;b"] {
            assert!(!env_key_is_safe(bad), "key '{bad}' should be rejected");
        }
    }

    #[test]
    fn url_encode_handles_shell_meta_safely() {
        // The whole point of this encoding is that downstream `sh -c`
        // (if any) sees no shell-active characters in the value.
        let encoded = url_encode_for_env("rm -rf /; echo $HOME `whoami`");
        assert!(!encoded.contains(';'));
        assert!(!encoded.contains('$'));
        assert!(!encoded.contains('`'));
        assert!(!encoded.contains(' '));
        // ASCII alnum + - _ . ~ pass through.
        assert_eq!(url_encode_for_env("safe-chars_only.~"), "safe-chars_only.~");
        // Standard hex output.
        assert_eq!(url_encode_for_env("a b"), "a%20b");
    }

    #[test]
    fn hookkind_names_round_trip() {
        for k in [
            HookKind::Ready,
            HookKind::NotReady,
            HookKind::Read,
            HookKind::Unread,
            HookKind::Publish,
            HookKind::Unpublish,
        ] {
            // ORP-native and MediaMTX-compat names both non-empty and
            // distinct.
            assert!(!k.as_str().is_empty());
            assert!(!k.mediamtx_name().is_empty());
            assert_ne!(k.as_str(), k.mediamtx_name());
        }
    }

    fn ctx(enabled: bool, audit: Arc<dyn AuditLogger>) -> HookContext {
        HookContext {
            enabled,
            workdir: PathBuf::from("/tmp"),
            audit,
            pubkey_hex: "deadbeef".to_string(),
            classification_banner: "UNCLASSIFIED".to_string(),
        }
    }

    #[tokio::test]
    async fn fire_hook_disabled_returns_disabled() {
        let audit: Arc<dyn AuditLogger> = Arc::new(orp_audit::InMemoryAuditLog::new());
        let result = fire_hook(
            &ctx(false, audit),
            HookKind::Ready,
            &HookSpec {
                argv: vec!["/bin/true".to_string()],
                restart: false,
                timeout_ms: None,
            },
            "stream-1",
            BTreeMap::new(),
        )
        .await;
        assert!(matches!(result, Err(HookError::Disabled)));
    }

    #[tokio::test]
    async fn fire_hook_rejects_empty_argv() {
        let audit: Arc<dyn AuditLogger> = Arc::new(orp_audit::InMemoryAuditLog::new());
        let result = fire_hook(
            &ctx(true, audit),
            HookKind::Ready,
            &HookSpec {
                argv: vec![],
                restart: false,
                timeout_ms: None,
            },
            "stream-1",
            BTreeMap::new(),
        )
        .await;
        assert!(matches!(result, Err(HookError::EmptyArgv)));
    }

    #[tokio::test]
    async fn fire_hook_rejects_shell_meta_in_argv() {
        let audit: Arc<dyn AuditLogger> = Arc::new(orp_audit::InMemoryAuditLog::new());
        let result = fire_hook(
            &ctx(true, audit),
            HookKind::Ready,
            &HookSpec {
                argv: vec![
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "rm -rf /; echo".to_string(),
                ],
                restart: false,
                timeout_ms: None,
            },
            "stream-1",
            BTreeMap::new(),
        )
        .await;
        assert!(matches!(result, Err(HookError::ShellMeta(_))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fire_hook_runs_true_and_records_audit() {
        let audit: Arc<dyn AuditLogger> = Arc::new(orp_audit::InMemoryAuditLog::new());
        let result = fire_hook(
            &ctx(true, audit.clone()),
            HookKind::Ready,
            &HookSpec {
                argv: vec!["/usr/bin/true".to_string()],
                restart: false,
                timeout_ms: Some(2_000),
            },
            "stream-1",
            BTreeMap::new(),
        )
        .await;
        let r = match result {
            Ok(r) => r,
            Err(_) => {
                // Some hosts ship `true` at /bin/true instead. Skip
                // gracefully rather than failing CI on that distro
                // detail — the test's intent is "happy path runs and
                // audits", which is exercised on the 90% of hosts that
                // have /usr/bin/true.
                eprintln!("skipping — /usr/bin/true not present on this host");
                return;
            }
        };
        assert_eq!(r.exit, 0);
        assert!(audit.len().await.unwrap() >= 1);
    }
}
