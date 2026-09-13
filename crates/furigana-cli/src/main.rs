//! `furigana` CLI バイナリ
//!
//! サブコマンド:
//! - `furigana lookup <text>`            : 1 ショット変換
//! - `furigana serve [--bind ...]`       : ローカル HTTP サーバー (Task #11)
//! - `furigana dict {add,list,remove,import,pull}` : 辞書管理 (Task #11)

mod commands;
mod config;
mod paths;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// 日本語フリガナ (ルビ) 解決ツール
#[derive(Parser, Debug)]
#[command(name = "furigana", version, about, long_about = None)]
struct Cli {
    /// データディレクトリを上書き (default: XDG / %LOCALAPPDATA%)
    #[arg(long, env = "FURIGANA_DATA_DIR", global = true)]
    data_dir: Option<PathBuf>,

    /// 設定ファイルを上書き (default: XDG/config/furigana/config.toml)
    #[arg(long, env = "FURIGANA_CONFIG", global = true)]
    config: Option<PathBuf>,

    /// 詳細ログ出力 (RUST_LOG=info 相当)
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 1 回だけ変換してそれを stdout に出す
    Lookup(commands::lookup::Args),

    /// 対話モード (REPL) で変換を試す
    Repl(commands::repl::Args),

    /// HTTP サーバーを起動
    Serve(commands::serve::Args),

    /// 辞書管理
    Dict(commands::dict::Args),
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    init_logging(cli.verbose);

    let paths = paths::Paths::resolve(cli.data_dir.as_deref(), cli.config.as_deref())?;
    let cfg = config::Config::load(&paths)?;

    tracing::debug!("data_dir: {}", paths.data_dir.display());
    tracing::debug!("config_file: {}", paths.config_file.display());

    match cli.command {
        Some(Commands::Lookup(args)) => commands::lookup::run(&args, &paths, &cfg),
        Some(Commands::Repl(args)) => commands::repl::run(args, &paths, &cfg),
        Some(Commands::Serve(args)) => commands::serve::run(args, &paths, &cfg),
        Some(Commands::Dict(args)) => commands::dict::run(args, &paths, &cfg),
        // 引数なしで起動された場合 (Windows で .exe をダブルクリック等) は REPL を立ち上げる。
        // help が見たければ `furigana --help`、終了は Ctrl-D / `:quit`。
        None => commands::repl::run(commands::repl::Args::default(), &paths, &cfg),
    }
}

fn init_logging(verbose: bool) {
    use tracing_subscriber::EnvFilter;
    let env_filter = if verbose {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
    };
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}
