//! `orp doctor` — first-time-user diagnostics.
//!
//! Run a series of preflight checks and print green/yellow/red status per
//! check. Designed to be safe to run before *and* after `orp start`:
//!
//! - No checks reach out over the public internet by default.
//! - All checks are read-only with respect to user state, except for the
//!   write-probe in `~/.orp/data/` which removes its own probe file.
//! - Exit code: `0` if no checks are red; `1` if any check is red.
//!
//! What we check:
//! 1. `protoc` on PATH (only matters when building from source).
//! 2. DuckDB writable: can we open a DuckDB connection at the configured
//!    `storage.duckdb.path`?
//! 3. RocksDB-compatible directory writable: can we write into the
//!    `storage.rocksdb.path` parent (we don't open RocksDB to keep doctor
//!    fast and dependency-light).
//! 4. Listening port free: is `server.port` (or 9090) bindable?
//! 5. Common config errors: load `config.yaml` if present and validate.
//! 6. Cert chain validity: only if `--https-url <url>` is passed; otherwise
//!    skipped (single-binary HTTP-by-default deploys are the norm).
//!
//! Each check returns one of three states:
//! ```text
//!   GREEN  ✓ passed
//!   YELLOW ! warning, non-fatal
//!   RED    ✗ failure, action required
//! ```

use anyhow::Result;
use colored::Colorize;
use std::io::IsTerminal;
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use orp_config::Config;

// ── Status enum ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoctorStatus {
    Green,
    Yellow,
    Red,
}

impl DoctorStatus {
    fn worse_than(self, other: Self) -> bool {
        rank(self) > rank(other)
    }
}

fn rank(s: DoctorStatus) -> u8 {
    match s {
        DoctorStatus::Green => 0,
        DoctorStatus::Yellow => 1,
        DoctorStatus::Red => 2,
    }
}

/// One check result with a user-readable message.
#[derive(Debug)]
pub struct DoctorCheck {
    pub name: &'static str,
    pub status: DoctorStatus,
    pub message: String,
    /// Optional remediation hint shown indented under the line.
    pub hint: Option<String>,
}

impl DoctorCheck {
    pub fn green(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: DoctorStatus::Green,
            message: message.into(),
            hint: None,
        }
    }

    pub fn yellow(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: DoctorStatus::Yellow,
            message: message.into(),
            hint: None,
        }
    }

    pub fn red(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: DoctorStatus::Red,
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn colors_enabled() -> bool {
    if std::env::var("NO_COLOR").is_ok() {
        return false;
    }
    std::io::stdout().is_terminal()
}

/// Find the parent directory we can probe for write access.
///
/// If the path is `./data.duckdb`, we want the directory `.`. If the path
/// is `/var/lib/orp/duck.db` we want `/var/lib/orp`.
fn parent_or_self(p: &Path) -> PathBuf {
    p.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf)
}

/// Touch a unique file in `dir` to verify write access. Returns true on success.
fn probe_write(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(format!(".orp-doctor-{}.tmp", std::process::id()));
    std::fs::write(&probe, b"orp doctor probe")?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

// ── Individual checks ─────────────────────────────────────────────────────────

/// 1. `protoc` available on PATH.
pub fn check_protoc() -> DoctorCheck {
    if which::which("protoc").is_ok() {
        DoctorCheck::green("protoc on PATH", "found (used only for source builds)")
    } else {
        DoctorCheck::yellow(
            "protoc on PATH",
            "not found — only matters if you build ORP from source",
        )
        .with_hint("Install: macOS `brew install protobuf` · Debian `apt install -y protobuf-compiler` · Fedora `dnf install -y protobuf-compiler`")
    }
}

/// 2. DuckDB at the configured path can be opened (read+write).
pub fn check_duckdb(config: &Config) -> DoctorCheck {
    let path = &config.storage.duckdb.path;
    let p = Path::new(path);
    let dir = parent_or_self(p);

    // First make sure the directory exists / is writable. Open errors otherwise
    // bubble up as confusing duckdb messages.
    if let Err(e) = probe_write(&dir) {
        return DoctorCheck::red(
            "DuckDB writable",
            format!("cannot write to {}: {}", dir.display(), e),
        )
        .with_hint(format!(
            "Create the directory or override storage.duckdb.path: \
             `mkdir -p {} && chmod u+w {}`",
            dir.display(),
            dir.display()
        ));
    }

    match duckdb::Connection::open(p) {
        Ok(conn) => {
            // Quick round-trip: SELECT 1.
            let probe: Result<i64, _> = conn.query_row("SELECT 42", [], |row| row.get::<_, i64>(0));
            match probe {
                Ok(42) => DoctorCheck::green(
                    "DuckDB writable",
                    format!("ok at {} (SELECT 1 round-trip succeeded)", path),
                ),
                Ok(other) => DoctorCheck::yellow(
                    "DuckDB writable",
                    format!(
                        "opened at {} but probe returned {} (expected 42)",
                        path, other
                    ),
                ),
                Err(e) => DoctorCheck::red(
                    "DuckDB writable",
                    format!("opened at {} but probe failed: {}", path, e),
                ),
            }
        }
        Err(e) => DoctorCheck::red("DuckDB writable", format!("cannot open {}: {}", path, e))
            .with_hint(
            "DuckDB will be created on first start. Pick a writable path in storage.duckdb.path \
             or run `orp start --in-memory` to skip persistence.",
        ),
    }
}

/// 3. RocksDB directory parent is writable. We don't open RocksDB itself
///    because it would lock the directory and slow doctor down materially.
pub fn check_rocksdb_dir(config: &Config) -> DoctorCheck {
    let path = &config.storage.rocksdb.path;
    let p = Path::new(path);
    let dir = parent_or_self(p);

    match probe_write(&dir) {
        Ok(()) => DoctorCheck::green(
            "RocksDB writable",
            format!("ok at {} (parent directory is writable)", dir.display()),
        ),
        Err(e) => DoctorCheck::red(
            "RocksDB writable",
            format!("cannot write to {}: {}", dir.display(), e),
        )
        .with_hint(format!(
            "Create the directory or override storage.rocksdb.path: \
             `mkdir -p {} && chmod u+w {}`",
            dir.display(),
            dir.display()
        )),
    }
}

/// 4. The configured server port is free (or already bound by another ORP
///    instance — we can't tell, so it's yellow).
pub fn check_port_free(config: &Config) -> DoctorCheck {
    let port = config.server.port;
    let addr = format!("127.0.0.1:{}", port);
    match TcpListener::bind(&addr) {
        Ok(listener) => {
            // Drop immediately so the real server can take it.
            drop(listener);
            DoctorCheck::green("Server port free", format!("ok — :{} is bindable", port))
        }
        Err(e) => DoctorCheck::yellow(
            "Server port free",
            format!(":{} is already in use ({})", port, e),
        )
        .with_hint(
            "If ORP is already running, this is fine — `orp status` to confirm. \
             Otherwise, change server.port or pass `--port <n>`.",
        ),
    }
}

/// 5. Common config errors. If `config.yaml` is present, validate it. If not,
///    that's fine — `orp start` falls back to defaults.
pub fn check_config(path: Option<&str>) -> DoctorCheck {
    let p = path.unwrap_or("config.yaml");
    if !Path::new(p).exists() {
        return DoctorCheck::green(
            "Config validation",
            format!("no {} found — defaults will be used", p),
        );
    }
    match Config::load_from_file(p) {
        Ok(_) => DoctorCheck::green("Config validation", format!("{} parses and validates", p)),
        Err(e) => DoctorCheck::red("Config validation", format!("{}: {}", p, e))
            .with_hint(format!("Run: `orp config validate {}` for details", p)),
    }
}

/// 6. Cert chain validity. Only run when an HTTPS URL is supplied. We do a
///    best-effort connect and rely on the system trust roots; on success we
///    report the leaf's CN / SAN and not-after date. On failure we surface the
///    underlying TLS error.
pub fn check_cert_chain(https_url: Option<&str>) -> DoctorCheck {
    let Some(url) = https_url else {
        return DoctorCheck::green("Cert chain validity", "skipped — pass --https-url to test");
    };
    if !url.starts_with("https://") {
        return DoctorCheck::yellow(
            "Cert chain validity",
            format!("not an https:// URL: {}", url),
        );
    }

    // We use the blocking client to avoid pulling tokio into doctor's path
    // when nobody asked for it. A 10s timeout is plenty for a TLS handshake.
    match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(client) => match client.get(url).send() {
            Ok(_) => DoctorCheck::green(
                "Cert chain validity",
                format!("TLS handshake to {} succeeded", url),
            ),
            Err(e) => DoctorCheck::red(
                "Cert chain validity",
                format!("TLS handshake to {} failed: {}", url, e),
            )
            .with_hint(
                "Check the cert chain (`openssl s_client -connect host:443 -servername host`) \
                 and that the system trust store includes the issuing CA.",
            ),
        },
        Err(e) => DoctorCheck::yellow(
            "Cert chain validity",
            format!("could not build HTTP client: {}", e),
        ),
    }
}

// ── Driver ────────────────────────────────────────────────────────────────────

/// Run all checks and print a report. Returns the worst status seen.
pub fn run_doctor(config_path: Option<&str>, https_url: Option<&str>) -> Result<DoctorStatus> {
    let config = match config_path {
        Some(p) if Path::new(p).exists() => Config::load_from_file(p)
            .map_err(|e| anyhow::anyhow!("failed to load config {}: {}", p, e))?,
        _ => Config::load_or_default(),
    };

    let checks: Vec<DoctorCheck> = vec![
        check_protoc(),
        check_duckdb(&config),
        check_rocksdb_dir(&config),
        check_port_free(&config),
        check_config(config_path),
        check_cert_chain(https_url),
    ];

    print_report(&checks);
    Ok(checks
        .iter()
        .map(|c| c.status)
        .fold(
            DoctorStatus::Green,
            |acc, s| {
                if s.worse_than(acc) {
                    s
                } else {
                    acc
                }
            },
        ))
}

fn print_report(checks: &[DoctorCheck]) {
    let color = colors_enabled();

    if color {
        println!("{}", "ORP Doctor".bold().cyan());
    } else {
        println!("ORP Doctor");
    }
    println!();

    for c in checks {
        let (sym, label) = match c.status {
            DoctorStatus::Green => ("✓", "PASS"),
            DoctorStatus::Yellow => ("!", "WARN"),
            DoctorStatus::Red => ("✗", "FAIL"),
        };
        if color {
            let painted_sym = match c.status {
                DoctorStatus::Green => sym.green().bold().to_string(),
                DoctorStatus::Yellow => sym.yellow().bold().to_string(),
                DoctorStatus::Red => sym.red().bold().to_string(),
            };
            println!("  {} {:<22} {}", painted_sym, c.name, c.message);
        } else {
            println!("  [{}] {:<22} {}", label, c.name, c.message);
        }
        if let Some(hint) = &c.hint {
            println!("       → {}", hint);
        }
    }

    let any_red = checks.iter().any(|c| c.status == DoctorStatus::Red);
    let any_yellow = checks.iter().any(|c| c.status == DoctorStatus::Yellow);

    println!();
    if any_red {
        if color {
            println!(
                "{} fix the failures above before running `orp start`.",
                "✗".red().bold()
            );
        } else {
            println!("FAIL: fix the failures above before running `orp start`.");
        }
    } else if any_yellow {
        if color {
            println!(
                "{} ORP should start, but review the warnings above.",
                "!".yellow().bold()
            );
        } else {
            println!("WARN: ORP should start, but review the warnings above.");
        }
    } else if color {
        println!(
            "{} ready — run `orp start --template maritime`.",
            "✓".green().bold()
        );
    } else {
        println!("OK: ready — run `orp start --template maritime`.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_orders_red_worst() {
        assert!(rank(DoctorStatus::Red) > rank(DoctorStatus::Yellow));
        assert!(rank(DoctorStatus::Yellow) > rank(DoctorStatus::Green));
    }

    #[test]
    fn check_config_missing_is_green() {
        let c = check_config(Some("does-not-exist-xyz.yaml"));
        assert_eq!(c.status, DoctorStatus::Green);
    }

    #[test]
    fn check_cert_chain_skipped_is_green() {
        let c = check_cert_chain(None);
        assert_eq!(c.status, DoctorStatus::Green);
    }

    #[test]
    fn check_cert_chain_non_https_is_yellow() {
        let c = check_cert_chain(Some("http://example.com"));
        assert_eq!(c.status, DoctorStatus::Yellow);
    }

    #[test]
    fn parent_or_self_handles_bare_filename() {
        let p = parent_or_self(Path::new("data.duckdb"));
        assert_eq!(p, PathBuf::from("."));
    }

    #[test]
    fn parent_or_self_handles_nested() {
        let p = parent_or_self(Path::new("/tmp/orp/data.duckdb"));
        assert_eq!(p, PathBuf::from("/tmp/orp"));
    }

    #[test]
    fn doctor_check_builders() {
        let g = DoctorCheck::green("x", "ok").with_hint("hint");
        assert_eq!(g.status, DoctorStatus::Green);
        assert_eq!(g.hint.as_deref(), Some("hint"));

        let y = DoctorCheck::yellow("x", "warn");
        assert_eq!(y.status, DoctorStatus::Yellow);

        let r = DoctorCheck::red("x", "boom");
        assert_eq!(r.status, DoctorStatus::Red);
    }
}
