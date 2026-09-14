//! `furigana lookup` サブコマンド
//!
//! 1 回だけ変換してそれを stdout に出して終了する CLI。
//! サーバー起動なし、即時 1 ショット用途。
//! 公開 API の `mode` パラメータと同じ 4 種に対応。

use crate::config::Config;
use crate::paths::Paths;
use anyhow::{bail, Context, Result};
use clap::Args as ClapArgs;
use furigana::{Furigana, RomajiStyle, TtsOptions};
use std::path::PathBuf;

/// `furigana lookup` のオプション
#[derive(ClapArgs, Debug)]
pub struct Args {
    /// 変換対象テキスト (`--batch` 指定時は省略する)
    text: Option<String>,

    /// stdin を 1 行 1 入力として読み、 1 行 1 結果を stdout に出す。
    /// 辞書の load が 1 回で済むので、 多数の入力をまとめて変換する時に使う
    /// (回帰テスト等)。 出力が複数行になる mode (`analyze` / `accent` /
    /// `--max-len` 付きの `aquestalk`) は行対応が壊れるので受け付けない。
    #[arg(long)]
    batch: bool,

    /// 変換モード: `tts` (default) | `hiragana` | `ruby` | `kanji` | `romaji` | `romaji-kunrei` |
    /// `analyze` | `accent` | `voicevox-aques` | `aquestalk` | `bouyomi`
    #[arg(short, long, default_value = "tts")]
    mode: String,

    /// dev / test 用: rules dir を直接指定 (= furigana-dict/rules/)。
    /// 指定時は `--data-dir/data/` スキャンを skip して raw dict 構造から直接 load する。
    /// `furigana dict pull` 配置済の通常運用では指定不要。
    #[arg(long)]
    rules_dir: Option<PathBuf>,

    /// dev / test 用: core dict dir を直接指定 (複数指定可、
    /// = furigana-dict/core/{jukugo, unihan, kanji, loanwords, works} 等)。
    /// 指定時は `--data-dir/data/` スキャンを skip する。
    #[arg(long)]
    core_dict_dir: Vec<PathBuf>,

    /// TTS: 「、」後に挿入する文字列
    #[arg(long, default_value = " ")]
    short_pause: String,

    /// TTS: 「。!?」後に挿入する文字列
    #[arg(long, default_value = "   ")]
    long_pause: String,

    /// TTS: `。` を残さず削除する
    #[arg(long)]
    drop_period: bool,

    /// TTS: 絵文字 / 顔文字パーツを読み上げから外す
    #[arg(long)]
    silence_symbols: bool,

    /// aquestalk: 無声化記号 `_` の自動付与を無効にする
    #[arg(long)]
    no_devoice: bool,

    /// aquestalk: 記号列をこの文字数以下のアクセント句単位へ分割し 1 行 1 塊で出力する
    /// (エンジン側の長さ上限対策。 0 = 分割しない)
    #[arg(long, default_value_t = 0)]
    max_len: usize,

    /// accent/analyze: dict bracket が無い token にも rule-based accent 推定を適用する
    /// (ADR-0007。 推定 phrase は `"estimated": true` で真値と区別される)
    #[arg(long)]
    estimate_accent: bool,
}

/// 実行
pub fn run(args: &Args, paths: &Paths, _cfg: &Config) -> Result<()> {
    if args.batch && args.text.is_some() {
        bail!("--batch と text 引数は同時に指定できません (入力は stdin から読みます)");
    }
    if args.batch && matches!(args.mode.as_str(), "analyze" | "accent") {
        bail!(
            "--batch は mode `{}` では使えません (出力が複数行になるため)",
            args.mode
        );
    }
    if args.batch && args.mode == "aquestalk" && args.max_len != 0 {
        bail!("--batch は `aquestalk` + `--max-len` では使えません (出力が複数行になるため)");
    }
    let f = if args.rules_dir.is_some() || !args.core_dict_dir.is_empty() {
        // dev/test override: raw furigana-dict/ 構造 (rules/ + core/<sub>/) から直接 load。
        // build_furigana の `<data_dir>/data/` flat スキャンを bypass して dev workflow を支える。
        let mut b = Furigana::builder();
        if let Some(rules) = &args.rules_dir {
            b = b.rules_dir(rules);
        }
        for core in &args.core_dict_dir {
            b = b.core_dict_dir(core);
        }
        b.estimate_accent(args.estimate_accent).build()?
    } else {
        super::furigana_builder(paths)
            .estimate_accent(args.estimate_accent)
            .build()?
    };

    if args.batch {
        use std::io::{BufRead, BufWriter, Write};
        let stdin = std::io::stdin();
        let mut out = BufWriter::new(std::io::stdout().lock());
        for line in stdin.lock().lines() {
            let line = line.context("read stdin")?;
            writeln!(out, "{}", convert(&f, &line, args)?).context("write stdout")?;
        }
        out.flush().context("flush stdout")?;
        return Ok(());
    }

    let text = args
        .text
        .clone()
        .context("変換対象テキストを指定してください (または --batch で stdin から読みます)")?;
    println!("{}", convert(&f, &text, args)?);
    Ok(())
}

/// 1 入力を mode に応じて変換する。 `lookup` と `--batch` で共有する。
fn convert(f: &Furigana, text: &str, args: &Args) -> Result<String> {
    let result = match args.mode.as_str() {
        "kanji" => text.to_string(),
        "ruby" => f.to_ruby(text),
        "hiragana" | "hira" => f.to_hiragana(text),
        "romaji" => f.to_romaji(text, RomajiStyle::Hepburn),
        "romaji-kunrei" | "kunrei" => f.to_romaji(text, RomajiStyle::Kunrei),
        // 棒読みちゃん (互換サーバー含む) へ流すテキストは tts mode と同一
        // (= 読み化 + pause 整形。 棒読みちゃん側の漢字誤読を bypass する)
        "tts" | "bouyomi" => {
            let opts = TtsOptions::default()
                .with_short_pause(args.short_pause.clone())
                .with_long_pause(args.long_pause.clone())
                .with_keep_period(!args.drop_period)
                .with_silence_symbols(args.silence_symbols);
            f.to_tts(text, &opts)
        }
        // VOICEVOX AquesTalk-風記法 (ADR-0001 adapter crate 経由)。
        // POST /accent_phrases?is_kana=true にそのまま渡せる。
        // dict bracket / --estimate-accent が無い token は平板 fallback。
        "voicevox-aques" | "voicevox" => {
            ja_furigana_voicevox::to_aques_kana(&f.to_accent(text))
        }
        // 本家 AquesTalk 音声記号列 (ADR-0001 adapter crate 経由)。
        // AquesTalk2 / AquesTalk10 の合成 API へそのまま渡せる。
        // VOICEVOX kana 記法との差分 = 半角 `?` / `。` と `、` の pause 区別 / 無声化 `_`。
        "aquestalk" => {
            let symbols = ja_furigana_aquestalk::to_aquestalk_with(
                &f.to_accent(text),
                ja_furigana_aquestalk::Options {
                    devoice: !args.no_devoice,
                    trailing_period: !args.drop_period,
                },
            );
            if args.max_len == 0 {
                symbols
            } else {
                // 1 行 1 塊 = そのまま逐次合成に流せる
                ja_furigana_aquestalk::split_for_aquestalk(&symbols, args.max_len).join("\n")
            }
        }
        // Smart engine debug API (★F1): AnalyzeResult を JSON pretty 出力。
        // alpha.10 段階の experimental、 path 採択 / 候補列 / boundary region を inspect 用途。
        "analyze" => {
            let result = f.analyze(text);
            serde_json::to_string_pretty(&result).context("serialize AnalyzeResult to JSON")?
        }
        "accent" => {
            let result = f.to_accent(text);
            serde_json::to_string_pretty(&result).context("serialize AccentResult to JSON")?
        }
        other => bail!(
            "未知の mode: {other} (使用可能: tts | bouyomi | hiragana | ruby | kanji | romaji | romaji-kunrei | analyze | accent | voicevox-aques | aquestalk)"
        ),
    };

    Ok(result)
}
