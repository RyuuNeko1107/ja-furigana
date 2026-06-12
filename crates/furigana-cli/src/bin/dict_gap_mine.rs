//! Dict gap mining tool (★dev / OSS curation use)。
//!
//! 入力 file (1 行 1 input) の各行に対し [`furigana::Furigana::analyze`] を流し、
//! band ≤ threshold の chunk を頻度集計する。 chunk は採択 path 上で隣接する
//! low-band token を kanji / alphabet 種別ごとに統合した連続区間。
//!
//! ## chunk merge (default)
//!
//! 隣接する low-band token を 1 chunk に統合して集計する。 これにより
//! 「言って」 「行った」 のような活用フレーズが、 「言っ」 「言わ」 「言う」 等の
//! **断片** ではなく 1 件として頻度集計される。 dict curation 判断
//! (= `[[kanji]]` block + `next_starts_any` 設計) に直結する。
//!
//! chunk 種別:
//! - **kanji**: 漢字含み low-band token + 後続の pure hiragana 送りがな
//!   (例: 「言って」 「行った」 「真っ暗」)
//! - **alphabet**: ASCII alphabetic 連続 low-band token (例: 「YouTube」
//!   「Twitter」 「Minecraft」 等、 loanwords dict 未登録のもの)
//!
//! `--per-token` で旧挙動 (= token 単位、 漢字含みのみ) に。
//!
//! ## 出力
//!
//! - `--output` (必須): main TSV
//!   (`count\tkind\tsurface\treading\texample_context`、 kanji + alphabet 混在)
//! - `--output-alphabet` (option): alphabet chunk のみ別 TSV
//!
//! ## 用途
//!
//! OSS dict curation の 「次に追加すべき surface」 ランキング作り:
//! - 実コメント / 実 web text の corpus を流す
//! - kanji_chunk を `core/jukugo/*.toml` / `core/kanji/overrides.toml` に
//! - alphabet_chunk を `core/loanwords/*.toml` に
//! - 何回も実行して残差を縮めていく

use anyhow::{Context, Result};
use clap::Parser;
use furigana::kana::{is_hiragana_char, is_kanji_char};
use furigana::{
    extract_dict_gap_candidates, surface_with_context, token_band, AnalyzeResult, Furigana,
};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::ops::Range;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(
    name = "furigana-dict-gap-mine",
    about = "Mine dict-gap chunks (kanji / alphabet) from a corpus and rank by frequency"
)]
struct Args {
    /// Input file (= 1 行 1 input)
    #[arg(long)]
    input: PathBuf,
    /// Main TSV output (= kanji + alphabet chunk 混在、 kind 列付き)
    #[arg(long)]
    output: PathBuf,
    /// Alphabet 専用 TSV output (option、 alphabet chunk のみ)
    #[arg(long)]
    output_alphabet: Option<PathBuf>,
    /// Base kanji 集計 TSV output (option)。 kanji chunk の漢字部分のみを key にして
    /// 集計、 「俺/俺は/俺も/俺の」 を 「俺」 で、 「言って/言わない/言うか」 を 「言」 で
    /// 集約する。 助詞 / 送りがな違いで分散した surface を base 漢字単位で見たい時用。
    #[arg(long)]
    output_kanji_base: Option<PathBuf>,
    /// rules dir (= furigana-dict/rules/)
    #[arg(long)]
    rules_dir: Option<PathBuf>,
    /// core dict dir (= furigana-dict/core/*、 複数指定可)
    #[arg(long)]
    core_dict_dir: Vec<PathBuf>,
    /// band threshold; この band 以下を 「dict 未登録疑い」 とみなす (default: 100)
    #[arg(long, default_value_t = 100)]
    threshold: u16,
    /// context chars (前後何文字、 default: 6)
    #[arg(long, default_value_t = 6)]
    context_chars: usize,
    /// 出力 surface 数の上限 (= top-N、 default: unlimited = 0)
    #[arg(long, default_value_t = 0)]
    top: usize,
    /// 旧挙動 = token 単位で集計 (= 「言っ」 「言わ」 を別 surface として count、
    /// alphabet は無視)。 既定は chunk merge (= kanji + alphabet 両方を chunk 化)。
    #[arg(long)]
    per_token: bool,
    /// progress を 1000 件単位で stderr に出す
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ChunkKind {
    Kanji,
    Alphabet,
}

impl ChunkKind {
    fn as_str(self) -> &'static str {
        match self {
            ChunkKind::Kanji => "kanji",
            ChunkKind::Alphabet => "alphabet",
        }
    }
}

#[derive(Debug)]
struct Chunk {
    kind: ChunkKind,
    surface: String,
    reading: String,
    range: Range<usize>,
}

struct Aggregate {
    count: u64,
    reading: String,
    example_context: String,
}

/// base-kanji 集計用 (= reading は base 単位だと無意味なので保存しない、
/// 代わりに代表 chunk surface 例を保存)。
struct BaseAggregate {
    count: u64,
    example_chunk: String,
    example_context: String,
}

/// ASCII 英字のみ (lib の `is_alphabet_char` と異なり **数字 / 全角 / 空白を含まない**:
/// alphabet chunk は数字で切る規則のため、 意図的に狭い判定を使う)。
fn is_ascii_alpha_char(c: char) -> bool {
    c.is_ascii_alphabetic()
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

/// 採択 path から chunk を抽出。
///
/// 規則:
/// - 起点 token: band ≤ threshold かつ (漢字含み or pure alphabet)
/// - 後続 token: 直前 token と range が隣接 + band ≤ threshold + kind 制約
///   - kanji chunk → 漢字含み or pure hiragana 送りがな
///   - alphabet chunk → pure alphabet のみ (= ひらがな / 数字で切れる)
/// - flush 条件: band > threshold / 種別不一致 / 隣接断絶 / 句読点・カタカナ・数字 token
fn chunk_low_band(result: &AnalyzeResult, threshold: u16) -> Vec<Chunk> {
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut cur: Option<Chunk> = None;

    for (i, token) in result.tokens.iter().enumerate() {
        let band = token_band(result, i).unwrap_or(u16::MAX);
        let is_low = band <= threshold;
        let s = &token.surface;
        if s.is_empty() {
            continue;
        }
        let has_kanji = s.chars().any(is_kanji_char);
        let is_pure_hira = s.chars().all(is_hiragana_char);
        let is_pure_alpha = s.chars().all(is_ascii_alpha_char);

        // この token をどう扱うか
        // - Some(Kanji): kanji chunk の member 候補 (= 漢字含み or 純ひらがな)
        // - Some(Alphabet): alphabet chunk の member 候補
        // - None: chunk breaker (= カタカナ / 数字 / 記号 / 高 band)
        let role: Option<ChunkKind> = if !is_low {
            None
        } else if has_kanji || is_pure_hira {
            Some(ChunkKind::Kanji)
        } else if is_pure_alpha {
            Some(ChunkKind::Alphabet)
        } else {
            None
        };

        let adjacent = cur
            .as_ref()
            .is_some_and(|c| c.range.end == token.range.start);

        let can_extend = match (&cur, role, adjacent) {
            (Some(c), Some(r), true) => c.kind == r,
            _ => false,
        };

        if can_extend {
            let c = cur.as_mut().unwrap();
            c.surface.push_str(s);
            c.reading.push_str(&token.reading);
            c.range.end = token.range.end;
        } else {
            if let Some(c) = cur.take() {
                if chunk_is_valid(&c) {
                    chunks.push(c);
                }
            }
            // 新 chunk 起点条件: kanji chunk は漢字含み、 alphabet chunk は alphabet
            // (= pure hiragana だけでは chunk 開始しない、 漢字 token を待つ)
            let start = match role {
                Some(ChunkKind::Kanji) if has_kanji => Some(ChunkKind::Kanji),
                Some(ChunkKind::Alphabet) => Some(ChunkKind::Alphabet),
                _ => None,
            };
            if let Some(kind) = start {
                cur = Some(Chunk {
                    kind,
                    surface: s.clone(),
                    reading: token.reading.clone(),
                    range: token.range.clone(),
                });
            }
        }
    }
    if let Some(c) = cur.take() {
        if chunk_is_valid(&c) {
            chunks.push(c);
        }
    }
    chunks
}

fn chunk_is_valid(c: &Chunk) -> bool {
    match c.kind {
        ChunkKind::Kanji => c.surface.chars().any(is_kanji_char),
        ChunkKind::Alphabet => c.surface.chars().any(is_ascii_alpha_char),
    }
}

fn record_chunk(
    aggregate: &mut HashMap<(ChunkKind, String), Aggregate>,
    chunk: &Chunk,
    line: &str,
    context_chars: usize,
) {
    let key = (chunk.kind, chunk.surface.clone());
    let entry = aggregate.entry(key).or_insert_with(|| {
        let ctx = surface_with_context(line, &chunk.range, context_chars);
        Aggregate {
            count: 0,
            reading: chunk.reading.clone(),
            example_context: format!("{}|{}|{}", ctx.before, ctx.surface, ctx.after),
        }
    });
    entry.count += 1;
}

/// chunk から漢字部分のみ抽出して base-kanji key にする。
/// 「俺は」 → 「俺」、 「真っ暗」 → 「真暗」、 「言って」 → 「言」、
/// 「お疲れ様でした」 → 「疲様」。
fn extract_kanji_base(surface: &str) -> String {
    surface.chars().filter(|c| is_kanji_char(*c)).collect()
}

fn record_kanji_base(
    aggregate: &mut HashMap<String, BaseAggregate>,
    chunk: &Chunk,
    line: &str,
    context_chars: usize,
) {
    if chunk.kind != ChunkKind::Kanji {
        return;
    }
    let base = extract_kanji_base(&chunk.surface);
    if base.is_empty() {
        return;
    }
    let entry = aggregate.entry(base).or_insert_with(|| {
        let ctx = surface_with_context(line, &chunk.range, context_chars);
        BaseAggregate {
            count: 0,
            example_chunk: chunk.surface.clone(),
            example_context: format!("{}|{}|{}", ctx.before, ctx.surface, ctx.after),
        }
    });
    entry.count += 1;
}

fn write_tsv(
    path: &PathBuf,
    entries: &[((ChunkKind, String), Aggregate)],
    include_kind_column: bool,
) -> Result<()> {
    let mut out = BufWriter::new(File::create(path).with_context(|| format!("create {path:?}"))?);
    if include_kind_column {
        writeln!(out, "count\tkind\tsurface\treading\texample_context")?;
    } else {
        writeln!(out, "count\tsurface\treading\texample_context")?;
    }
    for ((kind, surface), agg) in entries {
        let ctx = agg.example_context.replace(['\t', '\n'], " ");
        if include_kind_column {
            writeln!(
                out,
                "{}\t{}\t{}\t{}\t{}",
                agg.count,
                kind.as_str(),
                surface,
                agg.reading,
                ctx
            )?;
        } else {
            writeln!(out, "{}\t{}\t{}\t{}", agg.count, surface, agg.reading, ctx)?;
        }
    }
    out.flush()?;
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let started = Instant::now();
    let mode = if args.per_token {
        "per-token (legacy)"
    } else {
        "chunk (kanji + alphabet)"
    };
    eprintln!(
        "[info] mode = {mode}, threshold band ≤ {}, context_chars = {}",
        args.threshold, args.context_chars
    );

    let f = build_furigana(&args).context("Furigana build")?;
    eprintln!("[info] Furigana built in {:?}", started.elapsed());

    let file = File::open(&args.input).with_context(|| format!("open input {:?}", args.input))?;
    let reader = BufReader::new(file);

    let mut by_key: HashMap<(ChunkKind, String), Aggregate> = HashMap::with_capacity(50_000);
    let mut by_base: HashMap<String, BaseAggregate> = HashMap::with_capacity(10_000);
    let mut total_lines = 0u64;
    let mut total_chunks = 0u64;

    let scan_started = Instant::now();
    for (line_no, line) in reader.lines().enumerate() {
        let line = line.with_context(|| format!("read line {line_no}"))?;
        if line.is_empty() {
            continue;
        }
        total_lines += 1;
        let result = f.analyze(&line);

        if args.per_token {
            // 旧挙動: 漢字含み token を band threshold 以下で抽出 (alphabet 無視)
            for gap in
                extract_dict_gap_candidates(&result, &line, args.context_chars, args.threshold)
            {
                let chunk = Chunk {
                    kind: ChunkKind::Kanji,
                    surface: gap.surface.clone(),
                    reading: gap.reading.clone(),
                    range: gap.range.clone(),
                };
                total_chunks += 1;
                record_chunk(&mut by_key, &chunk, &line, args.context_chars);
                record_kanji_base(&mut by_base, &chunk, &line, args.context_chars);
            }
        } else {
            for chunk in chunk_low_band(&result, args.threshold) {
                total_chunks += 1;
                record_chunk(&mut by_key, &chunk, &line, args.context_chars);
                record_kanji_base(&mut by_base, &chunk, &line, args.context_chars);
            }
        }

        if args.verbose && total_lines.is_multiple_of(1000) {
            eprintln!(
                "[info] processed {total_lines} lines, {total_chunks} chunks, {} unique surfaces",
                by_key.len()
            );
        }
    }
    eprintln!(
        "[info] scan done in {:?}: {} lines, {} chunks, {} unique surfaces",
        scan_started.elapsed(),
        total_lines,
        total_chunks,
        by_key.len()
    );

    // sort by count desc
    let mut entries: Vec<((ChunkKind, String), Aggregate)> = by_key.into_iter().collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.1.count));
    if args.top > 0 && entries.len() > args.top {
        entries.truncate(args.top);
    }

    // main TSV (kind 列付き = kanji + alphabet 混在)
    write_tsv(&args.output, &entries, true)?;
    let kanji_n = entries
        .iter()
        .filter(|(k, _)| k.0 == ChunkKind::Kanji)
        .count();
    let alpha_n = entries
        .iter()
        .filter(|(k, _)| k.0 == ChunkKind::Alphabet)
        .count();
    eprintln!(
        "[info] wrote main TSV: {} surfaces ({} kanji + {} alphabet) → {:?}",
        entries.len(),
        kanji_n,
        alpha_n,
        args.output
    );

    // optional: alphabet 専用 TSV
    if let Some(alpha_path) = &args.output_alphabet {
        let alpha_only: Vec<_> = entries
            .iter()
            .filter(|(k, _)| k.0 == ChunkKind::Alphabet)
            .map(|((k, s), a)| {
                (
                    (*k, s.clone()),
                    Aggregate {
                        count: a.count,
                        reading: a.reading.clone(),
                        example_context: a.example_context.clone(),
                    },
                )
            })
            .collect();
        write_tsv(alpha_path, &alpha_only, false)?;
        eprintln!(
            "[info] wrote alphabet-only TSV: {} surfaces → {:?}",
            alpha_only.len(),
            alpha_path
        );
    }

    // optional: base-kanji 集計 TSV
    if let Some(base_path) = &args.output_kanji_base {
        let mut base_entries: Vec<(String, BaseAggregate)> = by_base.into_iter().collect();
        base_entries.sort_by_key(|e| std::cmp::Reverse(e.1.count));
        if args.top > 0 && base_entries.len() > args.top {
            base_entries.truncate(args.top);
        }
        let mut out = BufWriter::new(
            File::create(base_path).with_context(|| format!("create {base_path:?}"))?,
        );
        writeln!(out, "count\tkanji_base\texample_chunk\texample_context")?;
        for (base, agg) in &base_entries {
            let ctx = agg.example_context.replace(['\t', '\n'], " ");
            writeln!(
                out,
                "{}\t{}\t{}\t{}",
                agg.count, base, agg.example_chunk, ctx
            )?;
        }
        out.flush()?;
        eprintln!(
            "[info] wrote kanji-base TSV: {} bases → {:?}",
            base_entries.len(),
            base_path
        );
    }
    Ok(())
}
