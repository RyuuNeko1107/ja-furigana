# Contributing to furigana

このリポジトリは **engine (Rust 実装) 専用** です。

> **読みやルールデータの追加・修正は別 repo**:
> [`ja-furigana-dict/CONTRIBUTING.md`](https://github.com/RyuuNeko1107/ja-furigana-dict/blob/master/CONTRIBUTING.md) を参照。
> Rust 知識・git clone 不要、 GitHub Web UI で TOML 1 行追加から始められる。

## 1. 何を変更したい?

| 変更したいもの | PR 先 |
|---|---|
| **エンジン本体の改修 (Rust)** | このリポジトリ |
| **新しいルール schema (TOML 構造)** | このリポジトリ (`crates/furigana/src/rules/` モジュール) + furigana-dict 側のデータも合わせて |
| HTTP API の挙動変更 | このリポジトリ (`crates/furigana-cli/src/commands/serve/`) |
| 辞書バグ (誤読 / 読み追加 / 異体字 等) | [`ja-furigana-dict`](https://github.com/RyuuNeko1107/ja-furigana-dict) |
| engine bug | このリポジトリ |

## 2. Engine 改修の進め方

### 2-1. ローカル環境

[Rust の stable toolchain](https://rustup.rs/) が必要 (MSRV は `Cargo.toml` の `rust-version` 参照)。

```sh
git clone https://github.com/RyuuNeko1107/ja-furigana
cd ja-furigana

cargo build --all-targets
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p ja-furigana --example basic
```

### 2-2. テスト用 fixture

`crates/furigana/tests/fixtures/rules/` に `furigana-dict` のスナップショットコピーがあります。
スキーマ変更を伴う engine PR は、必要に応じてこのコピーも同期してください
(辞書データは `furigana-dict` 側がマスター)。

### 2-3. ルール schema を変更する場合

1. このリポジトリで `crates/furigana/src/rules/<file>.rs` の構造体を更新
2. `crates/furigana/tests/fixtures/rules/<file>.toml` も新フィールドに対応
3. 必要なら engine 側の利用コード (`scoring/` の provider / `numbers/` / `reading/`) も更新
4. **同時に** `furigana-dict` リポジトリに対応 PR を出す (新フィールドを使うデータがあるなら)
5. 両方の PR をリンクし、merge 順序を明示する

### 2-4. 「こういう変更ならここ」 早見表

| やりたいこと | 触る場所 |
|---|---|
| 公開 API (`Furigana` / `FuriganaBuilder`) | `api.rs` (本体)、 `api/builder.rs` (builder と辞書・規則のローダ) |
| 数字・日付・時刻・助数詞の読み | `scoring/numbers/` (下の表)。 読み方の **データ** は辞書側 (`counters.toml` ほか) |
| 数式 (`x²-5x+b=0`) | `scoring/numbers/math.rs` と `math/` |
| URL・絵文字・顔文字を保護する | `scoring/special/protect.rs` |
| 英字の読み (外来語辞書・passthrough) | `scoring/special/alphabet.rs` |
| 人名 (姓 + 名、 敬称・肩書きの手がかり) | `scoring/person_names.rs`、 手がかりの表は `scoring/person_names/cues.rs` |
| アクセント句の連結 | `chain_phrases.rs` と `chain_phrases/` (表 `table.rs` / ノード `nodes.rs` / 数字 `numerals.rs`) |
| 辞書 TOML の読み込み・検索 | `dict.rs`、 解析は `dict/parse.rs` |
| TOML の `[meta]` / role / schema_version | `loader/meta.rs`、 ディレクトリの列挙は `loader/walk.rs` |
| 入力の正規化 (全角半角・異体字・不可視文字) | `kana/normalize.rs` |
| 新しい規則データの schema | `rules/<file>.rs` (2-3 参照) |

### 2-5. モジュール構成

`crates/furigana/src/` 配下 (lib crate)。 各ファイルの先頭の `//!` に、 そのファイルの役割を書いてあります。

| 場所 | 役割 |
|---|---|
| `lib.rs` + `api.rs` + `api/builder.rs` | 公開 API。 解析は `scoring/pipeline.rs` 経由の薄い層 |
| `scoring/` | Smart engine (crate 内部)。 下の内訳 |
| `analyzer.rs` | Lindera ラッパ |
| `dict.rs` + `dict/parse.rs` | surface → reading 辞書 (unihan / rich entry / `[[kanji]]` block、 先頭 2 文字の prefix 区間引き) |
| `kana.rs` + `kana/normalize.rs` / `char_class.rs` | ひら⇄カタ・漢字判定 / 入力の正規化 / 文字種判定 (Unicode range 表の single home) |
| `numbers/` | 漢数字・助数詞の読み組み立て (digit / counter / extras / helpers)。 Smart engine 側の matcher は `scoring/numbers/` |
| `reading/` | 出力 layer (`ReadingToken` + `tokens_to_hiragana` / `tokens_to_ruby`)、 `output.rs` で surface 文字種ごとの reading 表記切替 |
| `chain_phrases.rs` + `chain_phrases/` / `accent_chain.rs` / `accent_digit*.rs` | アクセント句の組み立て (OpenJTalk 方式の連結規則) |
| `tts.rs` / `accent_symbols.rs` | TTS 整形 / TTS 記号列 adapter (voicevox / aquestalk crate) の共有コア |
| `romaji.rs` | ひらがな → ローマ字 (ヘボン式 / 訓令式) |
| `loader.rs` + `loader/` / `sanitize.rs` | TOML 汎用パーサ (schema_version 検証・role 推定) / 辞書 value の sanitize (制御文字・bidi override・過大長の entry を load 時に reject) |
| `rules/` | ルールデータ schema (counters / scales / units / days / symbols / numeric_phrases / compat / postprocess / emoji / models / context_cues) |
| `tablecache.rs` / `keyhash.rs` / `fxmap.rs` | アクセント表・連結表のキャッシュ / 表のキーのハッシュ / 決定的な高速 HashMap |
| `embedded.rs` / `error.rs` | 埋め込みデータ (空 rules) / エラー型 |

`scoring/` の内訳:

| 場所 | 役割 |
|---|---|
| `pipeline.rs` / `engine.rs` | 6 provider + Viterbi + post-pass の facade / Viterbi DP と `PathScore` |
| `candidate.rs` / `matcher.rs` / `boundary.rs` | candidate と band の型 / 辞書の条件 (`MatchCondition`) の判定 / 漢字連続の検出 |
| `dict_bridge.rs` / `lindera_fallback.rs` / `odoriji.rs` | 辞書 / Lindera / 踊り字 (々) の provider |
| `special.rs` + `special/` | 保護トークン (`protect.rs`) と英字 (`alphabet.rs`) の provider |
| `numbers/` | 数字系の provider。 `mod.rs` (適用順 `candidates_at`)、 `date.rs` `time.rs` `counter.rs` `numeric.rs` `symbols.rs` (種類ごとの matcher)、 `context.rs` (前後の文脈の判定)、 `patterns.rs` (regex の構築)、 `math.rs` + `math/` (数式) |
| `postpass.rs` / `names.rs` / `person_names.rs` + `person_names/` / `contextual.rs` / `paren_reading.rs` / `phonojoin.rs` | path 確定後に読みを補正する post-pass |
| `model_number.rs` / `digit_reading.rs` | 型番 (`models.toml`) / 英字語に付いた数字 |
| `bracket.rs` / `accent_estimate.rs` / `accent_lexicon.rs` | accent (bracket 記法、 規則推定、 アクセント専用の表) |
| `analyze.rs` / `explain.rs` / `inspect.rs` / `format.rs` | debug API (`analyze()`) / 読みの理由 / 辞書改善 log 用の検査 / 新 format の struct |
| `lattice.rs` | コスト lattice engine (opt-in の実験) |

他 crate: `crates/furigana-voicevox/` (VOICEVOX kana 記法 adapter)、 `crates/furigana-aquestalk/`
(AquesTalk 音声記号列 adapter)、 `crates/furigana-cli/` (`furigana` バイナリ + HTTP server
`commands/serve/`)。

### 2-6. コードの置き方 (保守しやすくするための約束)

- **1 ファイル 1 役割**。 目安は本体 800 行まで。 超えたら役割ごとに子 module に分ける (`api.rs` + `api/builder.rs` のように、 親は `mod` 宣言・ `pub use`・ 全体の流れだけ)
- **テストは `tests.rs` に分ける** (`#[cfg(test)] mod tests;`)。 本体を読む時にテストが邪魔にならないように
- **ファイルの先頭に `//!` で役割を書く**。 一覧は上の表と一致させる
- **規則は、 できるだけ辞書 (データ) 側で直せるようにする**。 読みや手がかり語の表を Rust に埋め込むのは、 辞書が無くても動かすための既定値に限る。 Rust 側の既定値を変えると辞書の無い環境の出力も変わるので、 ふつうは辞書側 (`furigana-dict`) を直す
- 構造を変えるだけの PR (ファイル分割・移動) は、 **ロジックを変えない** 、 **機能・修正と混ぜない**。 出力が変わっていないことは、 辞書 repo の corpus を前後で流して比べる

## 3. PR ガイドライン

### スコープを小さく

1 PR = 1 トピック。`feat: 新機能 + chore: リファクタ + fix: バグ修正` の
混在 PR ではなく、3 つに分けてください。

### コミットメッセージ

[Conventional Commits](https://www.conventionalcommits.org/) を推奨:

- `feat(numbers): 助数詞「篇」のルール追加`
- `fix(reading): 12時の読み修正 (シニジ → ジュウニジ)`
- `refactor(loader): generic 化で wrapper 削減`
- `docs(readme): クイックスタート例を追加`
- `ci: clippy のオプション強化`

### コードの場合

- `cargo fmt --all` を必ず通す (CI で `--check`)
- `cargo clippy --workspace --all-targets -- -D warnings` がクリーン
- 新規公開 API には rustdoc コメント (日本語可)
- 振る舞い変更を伴う場合 1 件以上のテストを添える

## 4. ステータス

0.x stable (crates.io で通常 publish 中、 最新は [CHANGELOG](CHANGELOG.md) 参照)。
公開 API / TOML スキーマを変える PR や構造を大きく変える PR は **Issue で先に相談**
してください。バグ修正・小さい機能追加は普通に PR で OK。
