//! 入力長スケーリング + allocation churn のベンチマーク。
//!
//! `lookup.rs` が「代表入力の latency」を測るのに対し、本ベンチは
//!
//! 1. **入力長スケーリング**: 同一段落の 1x / 4x / 16x 連結で ns/byte が一定か
//!    (= 超線形挙動が無いか) を見る
//! 2. **漢字連続 run**: 候補密度が高い (= Viterbi の edge 数が多い) 入力での挙動
//! 3. **allocation churn**: 1 回の `to_ruby` / `analyze` で起きる alloc 回数 / byte 数
//!    (= 投機的 Candidate の String clone コストの実測。最適化前後の比較指標)
//!
//! を測る。実行:
//!
//! ```sh
//! cargo bench -p ja-furigana --bench scaling
//!
//! # 実 dict mount (alloc 数は dict 規模依存なのでこちらが本番値):
//! FURIGANA_BENCH_CORE=../furigana-dict/core \
//! FURIGANA_BENCH_RULES=../furigana-dict/rules \
//!   cargo bench -p ja-furigana --bench scaling
//! ```
//!
//! alloc 数は criterion の統計ではなく起動時に 1 回 `[alloc]` 行で stderr 出力する
//! (counting allocator の delta 計測、 規模感の把握と前後比較用)。

// counting allocator (GlobalAlloc impl) はベンチ計測専用。 lib 本体には unsafe を
// 持ち込まない (workspace lint `unsafe_code = "deny"` は本 bench file のみ allow)。
#![allow(unsafe_code)]

use criterion::{criterion_group, BenchmarkId, Criterion, Throughput};
use furigana::Furigana;
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};

// ─── counting allocator ──────────────────────────────────────────────────────

static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);

/// System allocator の薄い wrapper、 alloc 回数 / byte 数を数えるだけ。
struct CountingAlloc;

// SAFETY: System への単純な委譲で、 layout / ptr の契約はそのまま満たされる。
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn alloc_snapshot() -> (u64, u64) {
    (
        ALLOC_COUNT.load(Ordering::Relaxed),
        ALLOC_BYTES.load(Ordering::Relaxed),
    )
}

// ─── 共通 fixture ────────────────────────────────────────────────────────────

/// lookup.rs と同じ構築 logic (実 dict は env var で mount)。
fn build_bench_furigana() -> Furigana {
    if let (Ok(core), Ok(rules)) = (
        std::env::var("FURIGANA_BENCH_CORE"),
        std::env::var("FURIGANA_BENCH_RULES"),
    ) {
        let f = Furigana::builder()
            .rules_dir(&rules)
            .core_dict_dir(&core)
            .build()
            .expect("build with real dict (FURIGANA_BENCH_CORE / _RULES)");
        f.preload().expect("preload analyzer");
        eprintln!("[bench] real dict mounted: {} entries", f.dict_size());
        return f;
    }

    let mut f = Furigana::minimal().expect("minimal init");
    let pairs: &[(&str, &str)] = &[
        ("灰桜", "ハイザクラ"),
        ("黎明", "レイメイ"),
        ("曙光", "ショコウ"),
        ("一期一会", "イチゴイチエ"),
        ("四面楚歌", "シメンソカ"),
        ("北海道", "ホッカイドウ"),
        ("吉祥寺", "キチジョウジ"),
        ("秋葉原", "アキハバラ"),
        ("今日", "キョウ"),
        ("明日", "アシタ"),
        ("一日", "イチニチ"),
        ("仲人", "ナコウド"),
    ];
    for (s, r) in pairs {
        f.add_reading(*s, *r);
    }
    f.preload().expect("preload analyzer");
    f
}

/// lookup.rs の "long" と同じ代表段落 (~430 bytes)。
const PARAGRAPH: &str =
    "今日は北海道の鹿児島と秋葉原で一期一会の出会いがあった。明日は仲人の家に行く。\
     灰桜の散る道を歩きながら、四面楚歌の状況をどう乗り越えるか考えた。\
     3冊の本と猫5匹を抱えて、5KMの距離を30分で走破した。\
     一日中、黎明から曙光が射すまで、テキストにふりがなを付けるという地味な作業を続けた。";

/// 漢字連続 run (候補密度が高く Viterbi edge 数が嵩む)。
const KANJI_RUN: &str =
    "東京特許許可局長今日急遽休暇許可拒否。国際連合教育科学文化機関本部事務局長代理。\
     全国高等学校野球選手権大会開会式典実行委員会。";

/// `base` を n 回連結した入力列 (1x / 4x / 16x) を返す。
fn scaled_inputs(base: &str) -> Vec<(String, String)> {
    [1usize, 4, 16]
        .iter()
        .map(|&n| {
            let text = base.repeat(n);
            (format!("{}x_{}B", n, text.len()), text)
        })
        .collect()
}

// ─── スケーリング benches ────────────────────────────────────────────────────

fn bench_scaling_paragraph(c: &mut Criterion) {
    let f = build_bench_furigana();
    let mut g = c.benchmark_group("scaling_paragraph");
    g.sample_size(30);
    for (label, text) in scaled_inputs(PARAGRAPH) {
        g.throughput(Throughput::Bytes(text.len() as u64));
        g.bench_with_input(BenchmarkId::new("to_ruby", &label), &text, |b, t| {
            b.iter(|| black_box(f.to_ruby(t)));
        });
        g.bench_with_input(BenchmarkId::new("tokenize", &label), &text, |b, t| {
            b.iter(|| black_box(f.tokenize(t)));
        });
    }
    g.finish();
}

fn bench_scaling_kanji_run(c: &mut Criterion) {
    let f = build_bench_furigana();
    let mut g = c.benchmark_group("scaling_kanji_run");
    g.sample_size(30);
    for (label, text) in scaled_inputs(KANJI_RUN) {
        g.throughput(Throughput::Bytes(text.len() as u64));
        g.bench_with_input(BenchmarkId::new("to_ruby", &label), &text, |b, t| {
            b.iter(|| black_box(f.to_ruby(t)));
        });
    }
    g.finish();
}

// ─── alloc churn レポート ────────────────────────────────────────────────────

/// 各入力で 1 回の `to_ruby` / `analyze` が起こす alloc 回数 / byte 数を stderr に出す。
///
/// 1 回目の呼び出しは lazy init (regex / prefix index) を含むため捨て、
/// 2 回目の delta を報告する。
fn report_alloc_churn(f: &Furigana) {
    eprintln!("[alloc] === allocation churn per call (count / bytes) ===");
    let mut cases: Vec<(String, String)> = scaled_inputs(PARAGRAPH)
        .into_iter()
        .map(|(l, t)| (format!("paragraph_{l}"), t))
        .collect();
    cases.extend(
        scaled_inputs(KANJI_RUN)
            .into_iter()
            .map(|(l, t)| (format!("kanji_run_{l}"), t)),
    );

    for (label, text) in &cases {
        let _ = black_box(f.to_ruby(text)); // warmup (lazy init を吸収)
        let (c0, b0) = alloc_snapshot();
        let _ = black_box(f.to_ruby(text));
        let (c1, b1) = alloc_snapshot();
        let _ = black_box(f.analyze(text)); // warmup 済み
        let (c2, b2) = alloc_snapshot();
        let _ = black_box(f.analyze(text));
        let (c3, b3) = alloc_snapshot();
        eprintln!(
            "[alloc] {label}: to_ruby = {} allocs / {} KiB, analyze = {} allocs / {} KiB",
            c1 - c0,
            (b1 - b0) / 1024,
            c3 - c2,
            (b3 - b2) / 1024,
        );
    }
}

criterion_group!(benches, bench_scaling_paragraph, bench_scaling_kanji_run);

fn main() {
    // alloc レポートは criterion 計測の前に 1 回だけ (統計ではなく規模感の把握用)。
    let f = build_bench_furigana();
    report_alloc_churn(&f);
    drop(f);

    benches();
    Criterion::default().configure_from_args().final_summary();
}
