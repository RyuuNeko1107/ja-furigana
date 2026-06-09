//! lookup 性能のベンチマーク。
//!
//! 実行:
//! ```sh
//! cargo bench -p ja-furigana --bench lookup
//! ```
//!
//! 結果は `target/criterion/<group>/<name>/` に HTML report として出力される。
//! 改善 PR ではベース → 修正後の比較を要約 commit すると差分が見える。

// `criterion::black_box` は 0.8 で deprecated になったため、std の方を使う
// (MSRV 1.66+ で利用可、本 crate は 1.88+)。これで criterion 0.5 / 0.8 両対応。
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use furigana::{Furigana, TtsOptions};
use std::hint::black_box;

/// ベンチ対象の [`Furigana`] を構築する。
///
/// 環境変数で **実 dict** を mount できる (= O(N×M) prefix scan のような dict 規模依存の
/// hot path を正確に測るため、 seed 12 entry では bottleneck が顕在化しない):
///
/// ```sh
/// FURIGANA_BENCH_CORE=../furigana-dict/core \
/// FURIGANA_BENCH_RULES=../furigana-dict/rules \
///   cargo bench -p ja-furigana --bench lookup
/// ```
///
/// 両 env が無ければ従来通り最小 seed (12 entry) で構築する。
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
    // 代表的な熟語をいくつか流し込んで「辞書 hit する」ケースも測れるようにする。
    // 実 seed (44k 字) は data ディレクトリ依存になるので最小サンプルに留める。
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

fn bench_init(c: &mut Criterion) {
    let mut g = c.benchmark_group("init");
    g.sample_size(20); // init は重いのでサンプル数控えめ
    g.bench_function("Furigana::minimal", |b| {
        b.iter(|| Furigana::minimal().expect("init"))
    });
    g.finish();
}

/// 実 dict 構築 (= TOML 全 file load + parse + index build) のトータル時間。
///
/// `FURIGANA_BENCH_CORE` / `_RULES` が両方セットされた時だけ走る。 未設定なら no-op
/// (= 最小 seed では測る意味が無いので skip)。 `preload` (Lindera init) も含めた
/// 「起動から最初の lookup 直前まで」 のトータルを測る。
fn bench_build(c: &mut Criterion) {
    let (Ok(core), Ok(rules)) = (
        std::env::var("FURIGANA_BENCH_CORE"),
        std::env::var("FURIGANA_BENCH_RULES"),
    ) else {
        return;
    };
    let mut g = c.benchmark_group("build");
    g.sample_size(10); // dict load は重い (~200ms)、 サンプル控えめ
    g.bench_function("real_dict_build", |b| {
        b.iter(|| {
            let f = Furigana::builder()
                .rules_dir(&rules)
                .core_dict_dir(&core)
                .build()
                .expect("build");
            black_box(f.dict_size())
        })
    });
    g.bench_function("real_dict_build_with_preload", |b| {
        b.iter(|| {
            let f = Furigana::builder()
                .rules_dir(&rules)
                .core_dict_dir(&core)
                .build()
                .expect("build");
            f.preload().expect("preload");
            black_box(f.dict_size())
        })
    });
    g.finish();
}

fn bench_lookup_mode(c: &mut Criterion) {
    let f = build_bench_furigana();
    let inputs: &[(&str, &str)] = &[
        ("short", "灰桜の散る道"),
        ("short_phrase", "一期一会と四面楚歌"),
        (
            "medium",
            "今日は北海道の鹿児島と秋葉原で一期一会の出会いがあった。明日は仲人の家に行く。",
        ),
        (
            "long",
            "今日は北海道の鹿児島と秋葉原で一期一会の出会いがあった。明日は仲人の家に行く。\
             灰桜の散る道を歩きながら、四面楚歌の状況をどう乗り越えるか考えた。\
             3冊の本と猫5匹を抱えて、5KMの距離を30分で走破した。\
             一日中、黎明から曙光が射すまで、テキストにふりがなを付けるという地味な作業を続けた。",
        ),
    ];

    let opts = TtsOptions::default();
    for (label, text) in inputs {
        let mut g = c.benchmark_group("lookup");
        g.throughput(Throughput::Bytes(text.len() as u64));
        g.bench_with_input(BenchmarkId::new("to_ruby", label), text, |b, t| {
            b.iter(|| black_box(f.to_ruby(t)));
        });
        g.bench_with_input(BenchmarkId::new("to_hiragana", label), text, |b, t| {
            b.iter(|| black_box(f.to_hiragana(t)));
        });
        g.bench_with_input(BenchmarkId::new("to_tts", label), text, |b, t| {
            b.iter(|| black_box(f.to_tts(t, &opts)));
        });
        g.finish();
    }
}

fn bench_tokenize(c: &mut Criterion) {
    let f = build_bench_furigana();
    let mut g = c.benchmark_group("tokenize");
    let short = "灰桜の散る道";
    let medium = "今日は5KMを30分で走った。一期一会の機会だった。";
    g.throughput(Throughput::Bytes(short.len() as u64));
    g.bench_function("short", |b| b.iter(|| black_box(f.tokenize(short))));
    g.throughput(Throughput::Bytes(medium.len() as u64));
    g.bench_function("medium", |b| b.iter(|| black_box(f.tokenize(medium))));
    g.finish();
}

/// `analyze()` raw API と `to_ruby()` 全段 pipeline (= analyze + ReadingToken
/// 変換 + tokens_to_ruby + postprocess) の latency 比較。 alpha.15 で Strict が
/// 削除されたので、 旧 Smart vs Strict 比較は無くなり、 raw analyze vs full
/// to_ruby のオーバーヘッド計測に役割が変化。
fn bench_analyze(c: &mut Criterion) {
    let f = build_bench_furigana();
    let inputs: &[(&str, &str)] = &[
        ("short", "灰桜の散る道"),
        ("short_phrase", "一期一会と四面楚歌"),
        (
            "medium",
            "今日は北海道の鹿児島と秋葉原で一期一会の出会いがあった。明日は仲人の家に行く。",
        ),
        (
            "long",
            "今日は北海道の鹿児島と秋葉原で一期一会の出会いがあった。明日は仲人の家に行く。\
             灰桜の散る道を歩きながら、四面楚歌の状況をどう乗り越えるか考えた。\
             3冊の本と猫5匹を抱えて、5KMの距離を30分で走破した。\
             一日中、黎明から曙光が射すまで、テキストにふりがなを付けるという地味な作業を続けた。",
        ),
    ];
    let mut g = c.benchmark_group("analyze");
    for (label, text) in inputs {
        g.throughput(Throughput::Bytes(text.len() as u64));
        g.bench_with_input(BenchmarkId::new("raw_analyze", label), text, |b, t| {
            b.iter(|| black_box(f.analyze(t)));
        });
        g.bench_with_input(BenchmarkId::new("to_ruby_full", label), text, |b, t| {
            b.iter(|| black_box(f.to_ruby(t)));
        });
    }
    g.finish();
}

criterion_group!(
    benches,
    bench_init,
    bench_build,
    bench_lookup_mode,
    bench_tokenize,
    bench_analyze
);
criterion_main!(benches);
