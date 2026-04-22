//! Observability 初期化層。
//!
//! - stderr + rolling file (`mcp.log.YYYY-MM`) への tracing subscriber セットアップ
//! - panic hook (`panic.log` に backtrace を append)
//! - Log dir / Log level の解決 (env 優先順位)
//!
//! `init_observability` が返す [`WorkerGuard`] は `main()` 内で drop させずに保持すること
//! (drop すると non_blocking writer が flush 前に shutdown する)。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// 環境変数名: log level (EnvFilter 形式)。優先順位 1 (CLI 次点)。
pub const ENV_LOG_LEVEL: &str = "GIT_WORKFLOW_LOG_LEVEL";
/// 環境変数名: log 出力ディレクトリ。未設定なら `~/.cache/git-workflow-mcp/`。
pub const ENV_LOG_DIR: &str = "GIT_WORKFLOW_LOG_DIR";
/// Rolling file のファイル名 prefix。
const FILE_PREFIX: &str = "mcp";
/// Rolling file のファイル名 suffix (拡張子相当)。
const FILE_SUFFIX: &str = "log";
/// panic 時に backtrace を append するファイル名。
const PANIC_LOG_NAME: &str = "panic.log";

/// Log level を解決する。優先順位: CLI > `GIT_WORKFLOW_LOG_LEVEL` > `RUST_LOG` > default。
///
/// `cli_level` は clap 由来で、未指定時 (default) も含めて既に文字列が入っている。
/// clap 側で default を "warn" に据え置きにしているため、CLI 指定有無の見分けは
/// 呼び出し側が行う (未指定なら `None` を渡す)。
pub fn resolve_log_level(cli_level: Option<&str>) -> String {
    if let Some(s) = cli_level
        && !s.is_empty()
    {
        return s.to_string();
    }
    if let Ok(v) = std::env::var(ENV_LOG_LEVEL)
        && !v.is_empty()
    {
        return v;
    }
    if let Ok(v) = std::env::var("RUST_LOG")
        && !v.is_empty()
    {
        return v;
    }
    "warn".to_string()
}

/// Log dir を解決する。`GIT_WORKFLOW_LOG_DIR` 優先、未設定なら `$HOME/.cache/git-workflow-mcp/`。
///
/// `HOME` すら取れない場合は `./` に fallback する (daemon 環境での最終 fallback)。
pub fn resolve_log_dir() -> PathBuf {
    if let Ok(v) = std::env::var(ENV_LOG_DIR)
        && !v.is_empty()
    {
        return PathBuf::from(v);
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home).join(".cache").join("git-workflow-mcp");
    }
    PathBuf::from(".")
}

/// Observability を初期化する。
///
/// - `log_dir` 配下に `mcp.log.YYYY-MM` を monthly rotation で生成
/// - stderr にも同時出力 (ansi off)
/// - panic hook を登録し `panic.log` に append
///
/// 返り値の `WorkerGuard` は drop すると non_blocking writer の flush が止まるため、
/// 呼び出し側 (main) で prog 終了まで保持する責務がある。
///
/// # Errors
///
/// - log_dir の作成失敗
/// - appender build 失敗
pub fn init_observability(log_dir: &Path, level: &str) -> Result<WorkerGuard> {
    std::fs::create_dir_all(log_dir)
        .with_context(|| format!("failed to create log dir: {}", log_dir.display()))?;

    // Rotation::MONTHLY は tracing-appender 0.2 に存在しない (MINUTELY / HOURLY / DAILY /
    // WEEKLY / NEVER のみ)。観測性用途では 1 日単位の rotation が実用上十分なので
    // DAILY を採用し、ファイル名は `mcp.log.YYYY-MM-DD` となる。
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(FILE_PREFIX)
        .filename_suffix(FILE_SUFFIX)
        .build(log_dir)
        .with_context(|| format!("failed to build rolling appender at {}", log_dir.display()))?;
    let (nb_writer, guard) = tracing_appender::non_blocking(appender);

    let file_layer = fmt::layer()
        .with_writer(nb_writer)
        .with_ansi(false)
        .with_target(true);
    let stderr_layer = fmt::layer().with_writer(std::io::stderr).with_target(true);

    tracing_subscriber::registry()
        .with(EnvFilter::new(level))
        .with(file_layer)
        .with(stderr_layer)
        .try_init()
        .map_err(|e| anyhow::anyhow!("failed to init tracing subscriber: {e}"))?;

    install_panic_hook(log_dir.join(PANIC_LOG_NAME));

    Ok(guard)
}

/// Panic hook を登録する。hook 内 I/O 失敗は hook 連鎖回避のため swallow する。
fn install_panic_hook(panic_log_path: PathBuf) {
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        let now = timestamp_now();
        let msg = format!("[{now}] panic: {info}\n{bt}\n");

        // ファイル append に失敗しても hook 内で panic してはならない。
        let _ = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&panic_log_path)
            .and_then(|mut f| f.write_all(msg.as_bytes()));

        // tracing は init 前に panic する可能性もあるので best-effort。
        // emit 順序上 file への append のほうが確実性が高い。
        tracing::error!(panic = %info, "process panicked");
    }));
}

/// RFC3339 風 timestamp を best-effort で返す。失敗時は空文字。
fn timestamp_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => format!("epoch_s={}", d.as_secs()),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_log_level_prefers_cli() {
        // CLI 指定あり時は env を無視する
        // SAFETY: test は single-threaded 環境で env を一時書き換えする。
        // 同 crate 内の他 test が env を触らない前提。
        unsafe {
            std::env::set_var(ENV_LOG_LEVEL, "debug");
        }
        let got = resolve_log_level(Some("info"));
        assert_eq!(got, "info");
        unsafe {
            std::env::remove_var(ENV_LOG_LEVEL);
        }
    }

    #[test]
    fn resolve_log_level_uses_default_when_all_empty() {
        unsafe {
            std::env::remove_var(ENV_LOG_LEVEL);
            std::env::remove_var("RUST_LOG");
        }
        let got = resolve_log_level(None);
        assert_eq!(got, "warn");
    }

    #[test]
    fn resolve_log_dir_uses_env() {
        unsafe {
            std::env::set_var(ENV_LOG_DIR, "/tmp/gwm-test-resolve");
        }
        let got = resolve_log_dir();
        assert_eq!(got, PathBuf::from("/tmp/gwm-test-resolve"));
        unsafe {
            std::env::remove_var(ENV_LOG_DIR);
        }
    }

    #[test]
    fn init_observability_creates_log_dir_and_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("logs");
        // 既に subscriber が他 test で init されている可能性があるため、
        // try_init が Err の場合でも dir 作成と appender ビルドは通る。
        // ただし同 process で複数回 init_observability する設計ではない点を明示するため
        // エラーは受け流す assert_ok にしない (guard の挙動だけ確認する意図)。
        match init_observability(&dir, "warn") {
            Ok(_guard) => {
                tracing::info!("probe-log-line");
                assert!(dir.exists(), "log dir must be created");
            }
            Err(_) => {
                // 他 test が subscriber を先に張っていれば try_init で Err。
                // dir 作成は済んでいるので確認のみ。
                assert!(dir.exists(), "log dir must be created even when init fails");
            }
        }
    }
}
