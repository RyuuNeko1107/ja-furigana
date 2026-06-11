//! Corpus regression test runner (★alpha.17、 expected match 計測用)。
//!
//! 形態素辞書 (IPADIC / UniDic) の比較や、 dict 改善前後の精度比較に使う。
//! `[[case]]` array の corpus TOML を input に取り、 各 case を `f.to_*` で実行、
//! `expected` field と照合して正解率を集計する。
//!
//! **Furigana 構築 (dict load + Lindera init) は 1 回だけ** で全 corpus を流すので、
//! 1 case ごとに CLI を起動する `tools/run_corpus.py` より 2 桁以上速い
//! (801 case で 数十秒 → 数秒)。 corpus は file / ディレクトリ (再帰 *.toml) を
//! 複数指定できる。
//!
//! ## 使い方
//!
//! ```bash
//! cargo run --release --bin furigana-corpus-check -- \
//!     --rules-dir <furigana-dict/rules> \
//!     --core-dict-dir <furigana-dict/core> \
//!     <furigana-dict/tests/corpus>          # dir 指定で配下 *.toml を一括
//! ```
//!
//! IPADIC / UniDic 比較は同じコマンドを feature flag を変えて 2 回 build し直す
//! (summary に morph dict 種別が出るので log の区別が付く):
//!
//! ```bash
//! cargo build --release --bin furigana-corpus-check
//! cargo build --release --bin furigana-corpus-check --no-default-features --features dict-unidic
//! ```

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use furigana::tts::TtsOptions;
use furigana::Furigana;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(
    name = "furigana-corpus-check",
    about = "Run corpora and report expected-match rate (single Furigana instance)"
)]
struct Args {
    /// Corpus TOML file / ディレクトリ (複数指定可、 dir は再帰で *.toml を収集)
    #[arg(required = true)]
    corpus: Vec<PathBuf>,
    /// Optional rules dir (= furigana-dict/rules/)
    #[arg(long)]
    rules_dir: Option<PathBuf>,
    /// Optional core dict dir (= furigana-dict/core/*、 複数指定可)
    #[arg(long)]
    core_dict_dir: Vec<PathBuf>,
    /// Print every case (default は failing case のみ)
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Debug, Deserialize)]
struct CorpusFile {
    #[serde(default, rename = "case")]
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    input: String,
    #[serde(default = "default_mode")]
    mode: String,
    #[serde(default)]
    expected: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    note: Option<String>,
}

fn default_mode() -> String {
    "ruby".into()
}

fn run_case(f: &Furigana, input: &str, mode: &str) -> Result<String> {
    Ok(match mode {
        "hiragana" => f.to_hiragana(input),
        "ruby" => f.to_ruby(input),
        "tts" => f.to_tts(input, &TtsOptions::default()),
        "romaji" => f.to_romaji(input, furigana::romaji::RomajiStyle::Hepburn),
        "kanji" => input.to_string(),
        other => return Err(anyhow!("unsupported mode: {:?}", other)),
    })
}

fn build_furigana(args: &Args) -> Result<Furigana> {
    let mut b = Furigana::builder();
    if let Some(rules) = &args.rules_dir {
        b = b.rules_dir(rules);
    }
    for d in &args.core_dict_dir {
        b = b.core_dict_dir(d);
    }
    b.build().context("build Furigana")
}

/// 指定 path 群を corpus file list に展開する (dir は再帰で `*.toml`)。
fn collect_corpus_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            collect_toml_in_dir(p, &mut out).with_context(|| format!("walk corpus dir: {p:?}"))?;
        } else {
            out.push(p.clone());
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn collect_toml_in_dir(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_toml_in_dir(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "toml") {
            out.push(path);
        }
    }
    Ok(())
}

/// 1 corpus file 分の集計。
#[derive(Default)]
struct Stats {
    cases: usize,
    expected_total: usize,
    correct: usize,
    errors: usize,
}

fn run_file(f: &Furigana, path: &Path, verbose: bool) -> Result<Stats> {
    let content = fs::read_to_string(path).with_context(|| format!("read corpus: {path:?}"))?;
    let corpus: CorpusFile =
        toml::from_str(&content).with_context(|| format!("parse corpus TOML: {path:?}"))?;

    let mut st = Stats {
        cases: corpus.cases.len(),
        ..Stats::default()
    };
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );

    for (i, case) in corpus.cases.iter().enumerate() {
        let actual = match run_case(f, &case.input, &case.mode) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error {name}#{i}: {e}");
                st.errors += 1;
                continue;
            }
        };
        if let Some(expected) = &case.expected {
            st.expected_total += 1;
            let pass = actual == *expected;
            if pass {
                st.correct += 1;
                if verbose {
                    println!("OK   {name}#{i}: {:?} -> {actual}", case.input);
                }
            } else {
                println!(
                    "FAIL {name}#{i}: input={:?} mode={:?}",
                    case.input, case.mode
                );
                println!("  expected: {expected:?}");
                // actual のみ Display で raw 出力 (= 全角 space を `\u{3000}` literal に
                // escape しない、 比較ツールから raw のまま読める)
                println!("  actual:   {actual}");
            }
        }
    }
    Ok(st)
}

fn run() -> Result<ExitCode> {
    let args = Args::parse();

    let files = collect_corpus_files(&args.corpus)?;
    if files.is_empty() {
        eprintln!("warning: no corpus TOML found");
        return Ok(ExitCode::SUCCESS);
    }

    let f = build_furigana(&args)?;

    let morph_dict = if cfg!(feature = "dict-unidic") {
        "unidic"
    } else {
        "ipadic"
    };

    let mut grand = Stats::default();
    let mut per_file: Vec<(String, Stats)> = Vec::new();
    for path in &files {
        let st = run_file(&f, path, args.verbose)?;
        grand.cases += st.cases;
        grand.expected_total += st.expected_total;
        grand.correct += st.correct;
        grand.errors += st.errors;
        per_file.push((path.display().to_string(), st));
    }

    println!();
    println!("=== Summary (morph dict: {morph_dict}) ===");
    for (name, st) in &per_file {
        if st.expected_total > 0 {
            let pct = (st.correct * 100) as f64 / st.expected_total as f64;
            println!("{name}: {}/{} ({pct:.1}%)", st.correct, st.expected_total);
        } else {
            println!("{name}: {} cases (expected なし)", st.cases);
        }
    }
    println!("Total cases:    {}", grand.cases);
    if grand.expected_total > 0 {
        let pct = (grand.correct * 100) as f64 / grand.expected_total as f64;
        println!(
            "Expected match: {}/{} ({pct:.1}%)",
            grand.correct, grand.expected_total
        );
    }
    if grand.errors > 0 {
        println!("Errors:         {}", grand.errors);
    }

    if grand.correct < grand.expected_total || grand.errors > 0 {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:?}");
            ExitCode::FAILURE
        }
    }
}
