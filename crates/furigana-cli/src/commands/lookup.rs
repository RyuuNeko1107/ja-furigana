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
    /// 変換対象テキスト
    text: String,

    /// 変換モード: `tts` (default) | `hiragana` | `ruby` | `kanji` | `romaji` | `romaji-kunrei` |
    /// `analyze` | `accent` | `voicevox-aques` | `bouyomi`
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

    /// accent/analyze: dict bracket が無い token にも rule-based accent 推定を適用する
    /// (ADR-0007。 推定 phrase は `"estimated": true` で真値と区別される)
    #[arg(long)]
    estimate_accent: bool,
}

/// 実行
pub fn run(args: Args, paths: &Paths, _cfg: &Config) -> Result<()> {
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

    let result = match args.mode.as_str() {
        "kanji" => args.text.clone(),
        "ruby" => f.to_ruby(&args.text),
        "hiragana" | "hira" => f.to_hiragana(&args.text),
        "romaji" => f.to_romaji(&args.text, RomajiStyle::Hepburn),
        "romaji-kunrei" | "kunrei" => f.to_romaji(&args.text, RomajiStyle::Kunrei),
        // 棒読みちゃん (互換サーバー含む) へ流すテキストは tts mode と同一
        // (= 読み化 + pause 整形。 棒読みちゃん側の漢字誤読を bypass する)
        "tts" | "bouyomi" => {
            let opts = TtsOptions {
                short_pause: args.short_pause,
                long_pause: args.long_pause,
                keep_period: !args.drop_period,
            };
            f.to_tts(&args.text, &opts)
        }
        // VOICEVOX AquesTalk-風記法 (ADR-0001 adapter crate 経由)。
        // POST /accent_phrases?is_kana=true にそのまま渡せる。
        // dict bracket / --estimate-accent が無い token は平板 fallback。
        "voicevox-aques" | "voicevox" => {
            ja_furigana_voicevox::to_aques_kana(&f.to_accent(&args.text))
        }
        // Smart engine debug API (★F1): AnalyzeResult を JSON pretty 出力。
        // alpha.10 段階の experimental、 path 採択 / 候補列 / boundary region を inspect 用途。
        "analyze" => {
            let result = f.analyze(&args.text);
            serde_json::to_string_pretty(&result).context("serialize AnalyzeResult to JSON")?
        }
        "accent" => {
            let result = f.to_accent(&args.text);
            serde_json::to_string_pretty(&result).context("serialize AccentResult to JSON")?
        }
        other => bail!(
            "未知の mode: {other} (使用可能: tts | bouyomi | hiragana | ruby | kanji | romaji | romaji-kunrei | analyze | accent | voicevox-aques)"
        ),
    };

    println!("{result}");
    Ok(())
}
