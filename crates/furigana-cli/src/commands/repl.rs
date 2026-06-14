//! `furigana repl` サブコマンド
//!
//! 対話モード。1 行入力すると現在の mode で変換して即時出力。
//! `:` プレフィクスでメタコマンド (`:help` で一覧)。
//!
//! 行編集は `rustyline` で:
//! - 矢印キーで履歴 / カーソル移動
//! - Tab でメタコマンド補完 (`:` プレフィックス時) と `:mode <name>` の候補補完
//! - Ctrl-C 1 回で readline キャンセル、Ctrl-D で終了
//! - 履歴は `<data_dir>/repl_history` に永続化

use crate::config::Config;
use crate::paths::Paths;
use anyhow::Result;
use clap::Args as ClapArgs;
use furigana::{Furigana, RomajiStyle, TtsOptions};
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{Context, Editor, Helper};
use std::time::Instant;
use unicode_width::UnicodeWidthStr;

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// 起動時の mode (default: `all`)
    #[arg(long, default_value = "all")]
    mode: String,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            mode: "all".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    All,
    Ruby,
    Hiragana,
    Tts,
    Kanji,
    Romaji,
    RomajiKunrei,
    Accent,
}

impl Mode {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "all" => Self::All,
            "ruby" => Self::Ruby,
            "hiragana" | "hira" => Self::Hiragana,
            "tts" => Self::Tts,
            "kanji" => Self::Kanji,
            "romaji" => Self::Romaji,
            "romaji-kunrei" | "kunrei" => Self::RomajiKunrei,
            "accent" => Self::Accent,
            _ => return None,
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Ruby => "ruby",
            Self::Hiragana => "hiragana",
            Self::Tts => "tts",
            Self::Kanji => "kanji",
            Self::Romaji => "romaji",
            Self::RomajiKunrei => "romaji-kunrei",
            Self::Accent => "accent",
        }
    }
}

/// メタコマンド名 (`:` なしの canonical 形)。`:` は optional でユーザーが
/// `help` でも `:help` でも打てるようにしている。
const META_COMMANDS: &[&str] = &[
    "debug", "help", "mode", "pull", "quit", "reload", "size", "tokens",
];

const MODE_NAMES: &[&str] = &[
    "all",
    "ruby",
    "hiragana",
    "tts",
    "kanji",
    "romaji",
    "romaji-kunrei",
    "accent",
];

/// 入力行の先頭がメタコマンドっぽいかを判定する用に、エイリアスも含めて拾う。
const META_COMMAND_ALIASES: &[&str] = &[
    "debug", "exit", "h", "help", "mode", "pull", "q", "quit", "r", "reload", "size", "tokens",
];

/// rustyline Helper: タブ補完だけ実装。highlight / hint / validate は default。
#[derive(Default)]
struct ReplHelper;

impl Completer for ReplHelper {
    type Candidate = Pair;
    fn complete(
        &self,
        line: &str,
        pos: usize,
        _: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let head = &line[..pos];

        // "<mode-cmd> <prefix>" → mode 候補。":" は optional。
        let mode_arg = head
            .strip_prefix(":mode ")
            .or_else(|| head.strip_prefix("mode "));
        if let Some(arg) = mode_arg {
            let start = pos - arg.len();
            let candidates = MODE_NAMES
                .iter()
                .filter(|m| m.starts_with(arg))
                .map(|m| Pair {
                    display: (*m).to_string(),
                    replacement: (*m).to_string(),
                })
                .collect();
            return Ok((start, candidates));
        }

        // ":<prefix>" → メタコマンド候補 (":" 付きで返す)
        if let Some(rest) = head.strip_prefix(':') {
            let candidates = META_COMMANDS
                .iter()
                .filter(|c| c.starts_with(rest))
                .map(|c| Pair {
                    display: format!(":{c}"),
                    replacement: format!(":{c}"),
                })
                .collect();
            return Ok((0, candidates));
        }

        // ASCII 英字 prefix → "<prefix>" → メタコマンド候補 (":" なしで返す)
        // 日本語など ASCII 以外を含むテキストでは補完しない (誤動作防止)
        if !head.is_empty() && head.chars().all(|c| c.is_ascii_alphabetic()) {
            let candidates = META_COMMANDS
                .iter()
                .filter(|c| c.starts_with(head))
                .map(|c| Pair {
                    display: (*c).to_string(),
                    replacement: (*c).to_string(),
                })
                .collect();
            return Ok((0, candidates));
        }

        Ok((pos, vec![]))
    }
}

impl Hinter for ReplHelper {
    type Hint = String;
}
impl Highlighter for ReplHelper {}
impl Validator for ReplHelper {}
impl Helper for ReplHelper {}

pub fn run(args: Args, paths: &Paths, _cfg: &Config) -> Result<()> {
    let Args { mode: initial_mode } = args;
    let mut f = super::build_furigana(paths)?;
    let mut mode = Mode::parse(&initial_mode).unwrap_or(Mode::All);
    let mut debug = false;

    let mut editor: Editor<ReplHelper, DefaultHistory> = Editor::new()?;
    editor.set_helper(Some(ReplHelper));
    let history_path = paths.data_dir.join("repl_history");
    let _ = editor.load_history(&history_path); // 無くても無視

    println!("furigana REPL  (dict_size: {})", f.dict_size());
    println!("  Tab で補完 / ↑↓ で履歴 / `help` でコマンド / `quit` で終了 (`:` は optional)");
    println!();

    // 初回起動 (辞書空) のとき :pull を提案
    if f.dict_size() == 0 {
        println!("辞書が未配置です。furigana-dict (~226 KB) を取得して使えるようにしますか？");
        if let Ok(ans) = editor.readline("[Y/n] > ") {
            let ans = ans.trim().to_ascii_lowercase();
            if ans.is_empty() || ans == "y" || ans == "yes" {
                if let Err(e) = run_pull_and_reload(paths, None, &mut f) {
                    eprintln!("pull failed: {e}");
                }
            } else {
                println!("(skipped — あとで `:pull` で取得できます)");
            }
        }
        // EOF / Ctrl-C は skip
        println!();
    }

    loop {
        let prompt = format!("{}> ", mode.as_str());
        let line = match editor.readline(&prompt) {
            Ok(l) => l,
            Err(ReadlineError::Interrupted) => {
                // Ctrl-C: 行をキャンセルして次の行へ
                continue;
            }
            Err(ReadlineError::Eof) => {
                // Ctrl-D: 終了
                break;
            }
            Err(e) => {
                eprintln!("read error: {e}");
                break;
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let _ = editor.add_history_entry(line);

        // メタコマンド判定: ":" は optional。先頭 token が既知コマンド名に
        // 完全一致した場合のみコマンド扱い (一致しなければ通常変換)。
        if let Some((cmd, arg)) = parse_meta(line) {
            handle_meta(cmd, arg, &mut mode, &mut debug, &mut f, paths);
            println!();
            continue;
        }

        // 通常変換
        let t0 = Instant::now();
        let tokens = f.tokenize(line);
        let t_tok = t0.elapsed();

        let t1 = Instant::now();
        match mode {
            Mode::All => {
                println!("  ruby:     {}", furigana::tokens_to_ruby(&tokens));
                println!("  hiragana: {}", furigana::tokens_to_hiragana(&tokens));
            }
            Mode::Ruby => println!("  {}", furigana::tokens_to_ruby(&tokens)),
            Mode::Hiragana => println!("  {}", furigana::tokens_to_hiragana(&tokens)),
            Mode::Tts => {
                let opts = TtsOptions::default();
                let hira = furigana::tokens_to_hiragana(&tokens);
                println!("  {}", furigana::tts::normalize_for_tts(&hira, &opts));
            }
            Mode::Kanji => println!("  {line}"),
            Mode::Romaji => {
                let hira = furigana::tokens_to_hiragana(&tokens);
                println!(
                    "  {}",
                    furigana::hiragana_to_romaji(&hira, RomajiStyle::Hepburn)
                );
            }
            Mode::RomajiKunrei => {
                let hira = furigana::tokens_to_hiragana(&tokens);
                println!(
                    "  {}",
                    furigana::hiragana_to_romaji(&hira, RomajiStyle::Kunrei)
                );
            }
            Mode::Accent => {
                let result = f.to_accent(line);
                if let Ok(json) = serde_json::to_string_pretty(&result) {
                    println!("  {json}");
                }
            }
        }
        let t_conv = t1.elapsed();

        if debug {
            println!(
                "  \x1b[2m[debug] tokenize {:.2}ms / convert {:.2}ms / total {:.2}ms\x1b[0m",
                t_tok.as_secs_f64() * 1000.0,
                t_conv.as_secs_f64() * 1000.0,
                (t_tok + t_conv).as_secs_f64() * 1000.0,
            );
        }
        println!(); // 結果と次 prompt の間に空行
    }

    let _ = editor.save_history(&history_path);
    Ok(())
}

/// 入力行をメタコマンドとして parse する。`:` は optional。
///
/// 先頭 token (`:` を除いたもの) が [`META_COMMAND_ALIASES`] に含まれる場合のみ
/// `Some((cmd, arg))` を返す。それ以外は `None` (通常テキスト変換に流す)。
/// これにより「灰桜」のような先頭が漢字の入力は確実に変換側に行く。
fn parse_meta(line: &str) -> Option<(&str, &str)> {
    let stripped = line.strip_prefix(':').unwrap_or(line);
    let mut parts = stripped.splitn(2, char::is_whitespace);
    let cmd = parts.next()?;
    let arg = parts.next().unwrap_or("").trim();
    if META_COMMAND_ALIASES.contains(&cmd) {
        Some((cmd, arg))
    } else {
        None
    }
}

fn handle_meta(
    cmd: &str,
    arg: &str,
    mode: &mut Mode,
    debug: &mut bool,
    f: &mut Furigana,
    paths: &Paths,
) {
    match cmd {
        "h" | "help" => print_help(),
        "q" | "quit" | "exit" => std::process::exit(0),
        "size" => println!("  dict_size: {}", f.dict_size()),
        "r" | "reload" => match super::build_furigana(paths) {
            Ok(new) => {
                *f = new;
                println!("  reloaded. dict_size: {}", f.dict_size());
            }
            Err(e) => println!("  reload failed: {e}"),
        },
        "pull" => {
            let version = if arg.is_empty() { None } else { Some(arg) };
            if let Err(e) = run_pull_and_reload(paths, version, f) {
                println!("  pull failed: {e}");
            }
        }
        "mode" => {
            if arg.is_empty() {
                println!("  current: {}", mode.as_str());
                println!("  available: {}", MODE_NAMES.join(" | "));
            } else if let Some(m) = Mode::parse(arg) {
                *mode = m;
                println!("  mode -> {}", mode.as_str());
            } else {
                println!("  unknown mode: {arg}");
            }
        }
        "debug" => {
            *debug = !*debug;
            println!("  debug: {}", if *debug { "on" } else { "off" });
        }
        "tokens" => {
            if arg.is_empty() {
                println!("  usage: :tokens <text>");
            } else {
                dump_tokens(f, arg);
            }
        }
        other => println!("  unknown command: :{other}  (try :help)"),
    }
}

/// `:pull` 共通実装: dict_pull::run → build_furigana で in-place 差し替え
fn run_pull_and_reload(
    paths: &Paths,
    version: Option<&str>,
    f: &mut Furigana,
) -> anyhow::Result<()> {
    super::dict_pull::run(paths, version)?;
    let new = super::build_furigana(paths)?;
    *f = new;
    println!("  reload 完了。dict_size: {}", f.dict_size());
    Ok(())
}

fn dump_tokens(f: &Furigana, text: &str) {
    let tokens = f.tokenize(text);
    if tokens.is_empty() {
        println!("  (no tokens)");
        return;
    }
    let surface_w = tokens
        .iter()
        .map(|t| UnicodeWidthStr::width(t.surface.as_str()))
        .max()
        .unwrap_or(0)
        .max(7);
    println!("  {:width$}  reading", "surface", width = surface_w);
    println!("  {:-<width$}  -------", "", width = surface_w);
    for t in &tokens {
        let pad = surface_w.saturating_sub(UnicodeWidthStr::width(t.surface.as_str()));
        let reading = t
            .reading
            .as_deref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "\x1b[2m(none)\x1b[0m".to_string());
        println!("  {}{}  {}", t.surface, " ".repeat(pad), reading);
    }
}

fn print_help() {
    println!("Commands (先頭の `:` は optional、`help` でも `:help` でも OK):");
    println!("  help          このヘルプ");
    println!("  mode <m>      mode 切替 (all|ruby|hiragana|tts|kanji|romaji|romaji-kunrei)  ※Tab で候補補完");
    println!("  debug         timing 表示の on/off (toggle)");
    println!("  tokens <text> 内部 token 配列を dump (なぜこの読み？を調べる用)");
    println!("  pull [vX.Y.Z] furigana-dict を取得 + 自動 reload");
    println!("  reload        data_dir から辞書を再 build");
    println!("  size          dict_size を表示");
    println!("  quit          終了 (Ctrl-D も可)");
    println!();
    println!("コマンド名と一致しない入力は現在の mode で変換して表示します。");
    println!("Tab: コマンド補完 / ↑↓: 履歴 / Ctrl-A,E,W,U: 行編集");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parse_roundtrip_and_aliases() {
        for name in [
            "all", "ruby", "hiragana", "tts", "kanji", "romaji", "romaji-kunrei", "accent",
        ] {
            assert_eq!(Mode::parse(name).map(Mode::as_str), Some(name), "roundtrip {name}");
        }
        assert_eq!(Mode::parse("hira").map(Mode::as_str), Some("hiragana"));
        assert_eq!(Mode::parse("kunrei").map(Mode::as_str), Some("romaji-kunrei"));
        assert!(Mode::parse("bogus").is_none());
    }

    #[test]
    fn parse_meta_recognizes_commands_only() {
        // 既知コマンド (`:` は optional)
        assert_eq!(parse_meta(":q"), Some(("q", "")));
        assert_eq!(parse_meta("quit"), Some(("quit", "")));
        assert_eq!(parse_meta(":mode hira"), Some(("mode", "hira")));
        assert_eq!(parse_meta("mode hira"), Some(("mode", "hira")));
        // 漢字先頭 / 通常テキストは None (= 変換側へ流す guard)
        assert_eq!(parse_meta("灰桜"), None);
        assert_eq!(parse_meta("猫が好き"), None);
        assert_eq!(parse_meta("hello"), None);
    }
}
