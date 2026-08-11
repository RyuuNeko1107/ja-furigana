# ja-furigana

[![CI](https://github.com/RyuuNeko1107/ja-furigana/actions/workflows/ci.yml/badge.svg)](https://github.com/RyuuNeko1107/ja-furigana/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/ja-furigana.svg)](https://crates.io/crates/ja-furigana)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/rust-1.89+-orange.svg)](https://www.rust-lang.org)

> A **data-driven local furigana / TTS-prep engine** for Japanese, in Rust.

日本語テキストに **フリガナ (読み仮名 / ルビ)** を付ける Rust 製ライブラリ + ローカル HTTP サーバー。
形態素解析 (Lindera + IPADIC) と TOML 辞書・ルールを組み合わせた **決定論的** エンジン。
TTS 音声合成の前段やふりがな補助での使用を想定。

[ryuuneko.com のフリガナ API](https://ryuuneko.com/?slug=furigana-api) の OSS 版として開発。
同等のインターフェース (`mode` / `text_b64` / `segmented` / `X-API-Key` 等) を提供するので、
既存クライアントからの差し替えやセルフホストに使える。

## 立ち位置と精度の限界

このプロジェクトは **「完全な日本語読み推定エンジン」 ではない**。

- ✅ **データ駆動のローカルふりがな / TTS 補助**:
  - VOICEVOX / OpenAI TTS 等の前段で「漢字を含む文 → ひらがな」を一括変換
  - Web / ブログ記事の `<ruby>` タグ自動生成
  - 配信テロップ用の難読語チェック
  - DB の人名・地名フィールドに読みフリガナを付与
  - **IT 用語の英単語にも対応** (Kubernetes / Docker / TypeScript 等を
    `core/loanwords/*.toml` で登録 → chunk 全体を完全一致 lookup)
- ❌ 苦手なこと:
  - **超高精度な文脈読み分け**: 機械学習ベース (BERT 等) のニューラル推論はしない
  - **辞書にない人名・固有名詞**: `furigana-dict` の手動 PR で語彙拡充が前提
  - **古文 / 文語 / 方言**: IPADIC ベースなので現代語が中心
  - **同形異音語の完璧な解決**: `rules/context/*.toml` でカバー範囲は限定的

「不確かなときは形態素解析の素朴な結果に fall back」「辞書 hit したものは確実に固定」 という
**保守的な決定論**。 コミュニティ PR で精度が上がる設計。

> **Status**: 0.3.0 stable (2026-08-11、 0.1.0 cut は 2026-05-12)。
> 0.3.0 = TTS adapter 2 本立て (VOICEVOX kana 記法 + 本家 AquesTalk 音声記号列) と
> 共有コアの lib 移設 (`furigana::accent_symbols`、 ADR-0009) + 顔文字/絵文字の
> TTS silent 化 (`TtsOptions::silence_symbols`)。
> (0.2.0 = intonation milestone: dict bracket notation 由来の accent 出力 (`--mode=accent`) +
> opt-in の rule-based accent 推定 (`--estimate-accent`) + VOICEVOX adapter crate
> (`ja-furigana-voicevox`))。
> **Smart engine** (= candidate scoring + Viterbi-like path 選択 + band lexicographic 比較) で
> 全 reading を解決。 6 provider 構成:
> ProtectToken (URL/Email/絵文字) / Alphabet passthrough / DictBridge (jukugo / unihan /
> `[[kanji]]` block の match) / NumberCandidate (数字 + 助数詞 / 大数 / SI / 日付) /
> Odoriji (踊り字 「々」 連濁) / LinderaFallback (band 50 safety net)。
>
> 精度 (= 0.1.0、 IPADIC default):
> - 主要 corpus 262 case: **99.2%**
> - OpenJTalk g2p 1000 件比較: **83-85%** (= seed 平均)
> - VOICEVOX engine query 1000 件比較: **75-77%** (= TTS 整合度)
>
> 形態素辞書は **`dict-ipadic`** (default) / **`dict-unidic`** (cwj) の feature flag で
> build-time switch 可能 (0.2.0 の A/B 評価で runtime は IPADIC 据え置き確定、
> UniDic の pitch accent は offline bracket 生成 tool [dict repo 側] で活用)。
>
> 詳細は [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) / 中長期計画は [docs/ROADMAP.md](./docs/ROADMAP.md) /
> 変更履歴は [CHANGELOG.md](./CHANGELOG.md) / breaking 変更ガイドは [MIGRATION.md](./MIGRATION.md)。
> 0.2.x patch では SemVer 互換維持 (= 公開 API / TOML スキーマ / CLI 引数 / HTTP レスポンス は additive only)。

## 名前の対応 (混乱しやすい点)

歴史的経緯で、 crate 名 / import 名 / バイナリ名がそれぞれ違う:

| 場面 | 名前 | 補足 |
|---|---|---|
| **crates.io の lib crate** | **`ja-furigana`** | `cargo add ja-furigana` |
| **lib の import 名 (Rust)** | **`furigana`** | `use furigana::Furigana;` ※`-` → `_` 慣例の例外 |
| **crates.io の CLI crate** | **`ja-furigana-cli`** | `cargo install ja-furigana-cli` |
| **インストール後のバイナリ名** | **`furigana`** | `furigana lookup ...` |
| **GitHub repo (本体)** | [`RyuuNeko1107/ja-furigana`](https://github.com/RyuuNeko1107/ja-furigana) | このリポジトリ |
| **GitHub repo (辞書)** | [`RyuuNeko1107/ja-furigana-dict`](https://github.com/RyuuNeko1107/ja-furigana-dict) | 辞書 PR はここに |

`furigana` という crate 名は別 OSS に先取りされていたため `ja-` prefix で公開しているが、
`[lib] name = "furigana"` 設定により `use ...` は `furigana` のまま。

## 1 分 Quickstart

### ライブラリとして使う

```toml
# Cargo.toml
[dependencies]
ja-furigana = "0.2.0"
# 形態素辞書を選びたい場合 (default = dict-ipadic):
# ja-furigana = { version = "0.2.0", default-features = false, features = ["dict-unidic"] }
```

```rust
use furigana::Furigana;  // crate 名は ja-furigana だが import 名は furigana

let mut f = Furigana::minimal()?;
f.add_reading("灰桜", "ハイザクラ");
println!("{}", f.to_ruby("灰桜の散る道"));
// → "{灰桜|はいざくら}の{散る|ちる}{道|みち}"
```

辞書 / ルールを mount する場合は builder API。 詳細は [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md#公開-api-lib)。

### production logging で辞書改善 (★alpha.19+)

production traffic から **dict 未登録 surface** を抽出して、 OSS curation loop に
PR 入力として流す pure 関数 API:

```rust
use furigana::{Furigana, extract_dict_gap_candidates};

let f = Furigana::builder().core_dict_dir("/path/to/data/core/jukugo").build()?;
let result = f.analyze(&user_input);
// band ≤ 100 (= unihan per-char or Lindera fallback) の漢字 token を context 込みで抽出
let gaps = extract_dict_gap_candidates(&result, &user_input, 3, 100);
for gap in gaps {
    log::info!(
        "dict-gap: surface={:?} reading={:?} band={} ctx=[{}|{}|{}]",
        gap.surface, gap.reading, gap.band,
        gap.context.before, gap.context.surface, gap.context.after,
    );
}
```

lib 自体は telemetry を持たない (= OSS ローカル完結方針)、 caller が log 形式 (JSON /
SQLite / Loki / Prometheus 等) を自由に選ぶ。 サンプル: [`examples/analyze_inspect.rs`](./crates/furigana/examples/analyze_inspect.rs)。

### CLI として使う

```sh
# インストール
cargo install ja-furigana-cli
# あるいは GitHub Releases から OS 別 binary を取得 (Windows なら exe ダブルクリックで REPL)

# 1 ショット変換
furigana lookup '灰桜の散る道'                   # → tts (default)
furigana lookup '灰桜の散る道' --mode ruby       # → {灰桜|はいざくら}...

# 出力ルール: 漢字 → ひらがな、 アルファベット / 数字 / 記号 → カタカナ統一
furigana lookup 'Anthropic の Claude を使う' --mode hiragana
# → アンソロピックのクロードをつかう

# 対話モード (引数なしで起動 = REPL)
furigana
# 中で :pull すれば furigana-dict を取得して dict_size が一気に増える

# HTTP サーバー
furigana serve                                  # http://127.0.0.1:8000
furigana serve --auto-pull                      # 起動時に GitHub Releases から最新辞書を取得
# config.toml に [auto_update] enabled=true / interval="24h" で background 定期更新

# Docker
docker run --rm -p 8000:8000 ghcr.io/ryuuneko1107/furigana:latest
```

## ドキュメント

| ドキュメント | 内容 |
|---|---|
| [`docs/HTTP_API.md`](./docs/HTTP_API.md) | endpoints / `mode` / エラー応答 / 認証 / hot reload / 他言語クライアント |
| [`docs/DATA_LAYOUT.md`](./docs/DATA_LAYOUT.md) | `<data_dir>` 構成 / `dict pull` の流れ / merge 順 |
| [`docs/CONFIG.md`](./docs/CONFIG.md) | `config.toml` / 環境変数 / CLI フラグ |
| [`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md) | crate 構成 / 内部モジュール / Smart engine 6 provider + Viterbi DP / 設計判断 |
| [`docs/ROADMAP.md`](./docs/ROADMAP.md) | Phase 計画 (CHANGELOG とは別、 未来志向) |
| [`CHANGELOG.md`](./CHANGELOG.md) | 完了履歴 (Keep a Changelog 形式) |
| [`CONTRIBUTING.md`](./CONTRIBUTING.md) | engine (Rust) PR ガイド |
| [`MAINTAINING.md`](./MAINTAINING.md) | release / publish / yank 手順 (メンテナー向け) |
| [`SECURITY.md`](./SECURITY.md) | 脆弱性報告窓口 |

辞書追加の PR は別 repo: [`ja-furigana-dict`](https://github.com/RyuuNeko1107/ja-furigana-dict) (Rust 不要、 TOML 1 行追加で完結)。
TOML schema の詳細は [`ja-furigana-dict/docs/SCHEMA.md`](https://github.com/RyuuNeko1107/ja-furigana-dict/blob/master/docs/SCHEMA.md) を参照。

## ライセンス

[MIT License](LICENSE)。 本リポジトリのコードのみ。
依存ライブラリのライセンスは [NOTICE.md](NOTICE.md) で保持
([`cargo-about`](https://github.com/EmbarkStudios/cargo-about) で自動生成、 CI で license drift 検知)。

## 主要依存 (Built with)

- 形態素解析: [`lindera`](https://github.com/lindera-morphology/lindera) + IPADIC (NAIST 由来、 BSD-3-clause-style)
- HTTP: [`axum`](https://github.com/tokio-rs/axum) / [`tokio`](https://github.com/tokio-rs/tokio) / [`reqwest`](https://github.com/seanmonstar/reqwest)
- CLI: [`clap`](https://github.com/clap-rs/clap) / [`rustyline`](https://github.com/kkawakam/rustyline)
- TOML / archive: [`serde`](https://github.com/serde-rs/serde) / [`toml`](https://github.com/toml-rs/toml) / [`flate2`](https://github.com/rust-lang/flate2-rs) / [`tar`](https://github.com/alexcrichton/tar-rs) / [`sha2`](https://github.com/RustCrypto/hashes)

詳細は [NOTICE.md](NOTICE.md)。

## コントリビュート

新しい読みやルール修正は、 ほとんどの場合 [`ja-furigana-dict`](https://github.com/RyuuNeko1107/ja-furigana-dict) の TOML を編集するだけ (Rust 不要)。
エンジン本体 (Rust) の改修は本リポジトリの [CONTRIBUTING.md](CONTRIBUTING.md) を参照。
