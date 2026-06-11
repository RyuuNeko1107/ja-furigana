# アーキテクチャ概要

ja-furigana の crate 構成と内部モジュールの役割。

> 戻る: [README](../README.md) / 関連: contributors 向けの編集ガイドは [CONTRIBUTING.md](../CONTRIBUTING.md)

## crate 構成

```
crates/
├── furigana/                 # lib crate (crates.io 名: ja-furigana / [lib] name = "furigana")
│   ├── lib.rs                # module 宣言 + 公開 API re-export (ファサード)
│   ├── api.rs                # Furigana 構造体 + FuriganaBuilder (公開 API のエントリ)
│   ├── analyzer.rs           # Lindera + IPADIC のラッパ (Smart engine の fallback として使用)
│   ├── kana.rs               # ひら⇄カタ + Unicode 正規化
│   ├── dict.rs               # surface→reading 辞書 (jukugo ≥2 文字 / unihan 1 文字 / [[kanji]] block / Detailed Entry を多重保持)
│   ├── sanitize.rs           # 辞書 load 経路の sanitize layer (任意コード埋め込み防御 — 制御文字 / bidi override / zero-width / 過大長 reject)
│   ├── tts.rs                # TTS 整形 (normalize_for_tts) + segment_for_tts
│   ├── romaji.rs             # ひらがな → ローマ字 (ヘボン式 / 訓令式)
│   ├── error.rs              # FuriganaError / Result
│   ├── loader.rs             # TOML 汎用 parser (parse_toml<T> + load_or_default<T>)
│   ├── embedded.rs           # 空 default RulesData (rules は furigana-dict 側)
│   ├── rules/                # データスキーマ
│   │   ├── counters.rs       #   counters/*.toml
│   │   ├── days.rs           #   days.toml (1〜31 日特殊読み)
│   │   ├── scales.rs         #   scales.toml (大数: 万 / 億 / 兆…)
│   │   ├── units.rs          #   units.toml (km / kg / 円 / % …)
│   │   ├── symbols.rs        #   symbols.toml (+/-/% etc)
│   │   ├── numeric_phrases.rs #  数詞慣用語句 data (scoring/numbers の try_phrase が consume)
│   │   ├── compat.rs         #   compat.toml (異体字)
│   │   └── postprocess.rs    #   postprocess.toml (mode 別後処理 regex 置換)
│   ├── numbers/              # 数値処理 (data-driven、 scoring/numbers.rs から呼ばれる)
│   │   ├── helpers.rs        #   zen2han / norm_num / sokuonize_last / kansuji_to_arabic 等
│   │   ├── digit.rs          #   number_to_katakana
│   │   ├── counter.rs        #   euphonic_counter_read
│   │   └── extras.rs         #   scale / si_unit / symbol 単発読み
│   ├── reading/              # ReadingToken + output helper
│   │   ├── mod.rs            #   ReadingToken (= Smart engine Token の API 互換 wrapper)
│   │   └── output.rs         #   tokens_to_hiragana / tokens_to_ruby
│   └── scoring/              # Smart engine 本体
│       ├── mod.rs            #   module 集約
│       ├── pipeline.rs       #   ★Pipeline facade — provider 構成 + Viterbi + Reading Post-pass を所有する single seam
│       ├── candidate.rs      #   Score / Candidate / CandidateProvider trait + ScoringContext + band 定数
│       ├── engine.rs         #   PathScore (weakest_band agg) + solve_path Viterbi DP
│       ├── boundary.rs       #   BoundaryAnalysis (b)(c) 漢字連続 penalty
│       ├── format.rs         #   Entry / EntryDetail / MatchBlock / KanjiBlock の dict 受け入れ型
│       ├── matcher.rs        #   MatchContext + matches_context() + classify_char() + resolve_readings
│       ├── special.rs        #   ProtectTokenProvider (URL/Email/絵文字) + AlphabetPassthroughProvider (loanwords lookup 込)
│       ├── dict_bridge.rs    #   DictBridgeProvider (jukugo / unihan / [[kanji]] block、 先頭 char prefix index 引き)
│       ├── numbers/          #   NumberCandidateProvider (band 950)。 patterns.rs = regex 構築、 mod.rs = 候補種別ごとの try_* (日付 / 時刻 / scale / SI / 助数詞 / 数詞慣用語句 / 記号 / 素数字)
│       ├── odoriji.rs        #   OdorijiProvider (々 placeholder) + RendakuPass
│       ├── contextual.rs     #   HaraSukuPass (腹+空く 2-token-back 補正)
│       ├── postpass.rs       #   ReadingPostPass trait + POST_PASSES 配列 (path 確定後の token 補正 seam)
│       ├── lindera_fallback.rs #   LinderaFallbackProvider (band 50/150 safety net + gap-passthrough)
│       ├── bracket.rs        #   bracket notation parse → AccentPhrase (0.2.0 core)
│       ├── analyze.rs        #   AnalyzeResult / Token + analyze() / analyze_tokens()
│       └── inspect.rs        #   dict gap 抽出等の inspection helper (公開 re-export)
│
└── furigana-cli/             # bin crate (crates.io 名: ja-furigana-cli / バイナリ名: furigana)
    └── src/
        ├── main.rs           # clap dispatch (引数なしなら repl にフォールバック)
        ├── bin/              # dev tool 群: furigana-corpus-check (corpus 一括 regression) /
        │                     #   furigana-analyze-one (AnalyzeResult dump) / furigana-dict-gap-mine
        ├── paths.rs          # 実行ファイル横を default、--data-dir / FURIGANA_DATA_DIR で上書き
        ├── config.rs         # config.toml ロード ([server] / [auth].tokens / .admin_tokens)
        └── commands/
            ├── mod.rs        # build_furigana (各コマンドが共有する Furigana 組み立て)
            ├── lookup.rs     # furigana lookup
            ├── repl.rs       # furigana repl (rustyline + Tab 補完 + 履歴)
            ├── dict.rs       # furigana dict {add,list,remove,import,pull}
            ├── dict_pull.rs  #   pull 実装 (GitHub Releases + SHA-256 検証 + tar 展開)
            └── serve/        # furigana serve (Axum HTTP)
                ├── mod.rs    #   run() + Args + shutdown_signal + SIGHUP reload
                ├── handlers.rs # /furigana / /healthz / /admin/reload + do_reload
                ├── auth.rs   #   X-API-Key / Bearer middleware (一般 + admin) + CORS
                └── types.rs  #   FuriganaParams / FuriganaResponse / AppState
```

## 公開 API (lib)

```rust
use furigana::{Furigana, FuriganaBuilder, RomajiStyle, TtsOptions};

let mut f = Furigana::minimal()?;          // Lindera は lazy init
f.add_reading("灰桜", "ハイザクラ");        // 動的に辞書追加
f.preload()?;                              // (option) Lindera を eager init

let _ = f.tokenize("灰桜の道");
let _ = f.to_ruby("灰桜の道");
let _ = f.to_hiragana("灰桜の道");
let _ = f.to_tts("灰桜の道", &TtsOptions::default());
let _ = f.to_romaji("灰桜の道", RomajiStyle::Hepburn);
let _ = f.segment_tts("...", &TtsOptions::default(), 60);
let _ = f.dict_size();
f.merge_dict_toml(r#"[entries]
"黎明" = "レイメイ""#)?;
```

`FuriganaBuilder`:

```rust
let f = Furigana::builder()
    .rules_dir("/path/to/data")           // 単一上書き
    .core_dict_dir("/path/to/data")       // 複数追加可
    .user_dict_dir("/path/to/data/user")  // 複数追加可
    .overrides_file("/path/to/data/overrides.toml")  // 複数追加可
    .add_entry("追加語", "ツイカゴ")      // 最優先
    .build()?;
```

## パイプライン (alpha.15+、 Smart engine 一本化)

`Furigana::to_*` の流れ:

工程 2〜4 は `scoring::pipeline::Pipeline` (facade) が所有する — provider の追加・
順序変更・post-pass 追加はこの 1 module の変更で完結する。

| # | 工程 | 実装場所 |
|---|---|---|
| 1 | テキスト正規化 (NFKC + 互換マップ) | `kana::normalize_text` + `rules::compat` |
| 2 | candidate edge 列挙 (6 provider) | `scoring::pipeline` → `scoring::analyze` (下記) |
| 3 | Viterbi DP で path 解 | `scoring::engine::solve_path` |
| 4 | Reading Post-pass (連濁 RendakuPass + 腹空く HaraSukuPass、 適用順は `POST_PASSES` 配列) | `scoring::postpass::apply_all` |
| 5 | `AnalyzeToken` → `ReadingToken` 変換 | `Furigana::tokenize` |
| 6 | surface 文字種で reading 表記を分岐 | `reading::output::tokens_to_hiragana` (下記) |
| 7 | 後処理 regex 置換 (mode 別) | `rules::postprocess::PostProcessData::apply` |
| 8 | TTS 正規化 (`tts` mode のみ) | `tts::normalize_for_tts` |

### Step 2 の詳細: 6 provider の candidate 提供

`Furigana::analyze` は input を 6 つの provider に流し、 各 byte 位置で
**band 付き candidate edge** を列挙する:

| 優先 (band) | provider | 役割 | 例 |
|---|---|---|---|
| 2000 | `ProtectTokenProvider` | URL / Email / 絵文字 (= 読み付けず passthrough) | `https://example.com` / `a@b.jp` / 🦀 |
| 1000 | `DictBridgeProvider` (jukugo) | dict surface ≥2 字 | `灰桜` → `ハイザクラ` |
| 1000 / 100 | `AlphabetPassthroughProvider` | 英字 passthrough (lookup あり = band 1000、 無し = band 100) | `Kubernetes` |
| 950 | `NumberCandidateProvider` | 数字 + 助数詞 / 大数 / SI / 日付 / 時刻 / 記号 / 数詞慣用語句 | `1万円` / `2025年10月30日` / `100km` / `二十歳` |
| 100 | `DictBridgeProvider` (unihan / `[[kanji]]`) | 1 字 surface fallback + 文脈分岐 | `米` (= 次がひらがな → こめ / 漢字 → ベイ) |
| 100 | `OdorijiProvider` | 「々」 placeholder edge (post-pass で連濁適用) | `山々` → `やまやま` |
| 50 | `LinderaFallbackProvider` | 上記 5 が一切覆わない位置の safety net | 助詞 / okurigana / dict 未登録 単語 |

### Step 3 の詳細: `PathScore` lexicographic 比較

`solve_path` は input 全体を覆う path のうち最良のものを選ぶ。 比較は **discrete
band の lexicographic** で、 連続値 score の calibration 沼を回避:

1. **`weakest_band` 大**: path 中の最低 band edge が高いほど勝ち (= 弱い edge を含まない)
2. **`edge_count` 少**: 同 weakest_band なら edges が少ない方が勝ち (= longest match 優先)
3. **`total_match_hits` 多**: 文脈 match 条件 hit 数で tie-break
4. **`total_boundary_penalty` 軽** (= less negative): 漢字連続境界 penalty 軽い方が勝ち

これにより 「米国産」 (3 字 jukugo = 1 edge band 1000) は per-char fallback
「米+国+産」 (3 edges, weakest=100) に勝つ。

### Step 6 の詳細: `tokens_to_hiragana` の出力ルール

token の **surface 文字種** で reading 表記を切り替える:

- **漢字を含む surface** → reading をひらがな化 (`kata_to_hira`)
  - 例: 「灰桜」 + ハイザクラ → 「はいざくら」 / 「3本」 + サンボン → 「さんぼん」
- **surface == reading (kana 等価)** → **surface をそのまま** 保持 (= Lindera fallback が
  ひらがな助詞 / okurigana に reading=カタカナ を付けて返す case)
  - 例: 「の」 + 「ノ」 → 「の」 (kata_to_hira normalize で等価判定)
- **漢字を含まない surface & reading が surface と非等価** → reading を **カタカナに統一**
  (`hira_to_kata`、 alphabet loanword の phonetic reading 用)
  - 例: 「Kubernetes」 + クバネティス → 「クバネティス」
  - 例: 「C++」 + シープラスプラス → 「シープラスプラス」

## Lindera の lazy init

`Furigana::minimal()` / `FuriganaBuilder::build()` の時点では Analyzer を init せず、最初の `tokenize` / `to_*` 呼び出し時に [`OnceLock`] で 1 度だけ init される。

`Furigana::minimal()` 単体の bench で **5.97 ms → 27.3 µs (-99.5%)**。CLI レベルでは `--version` / `--help` 等の Lindera 不要経路が ~80 ms → ~10 ms に高速化。`furigana serve` は `Furigana::preload()` を listen 前に呼んでいるので、最初のリクエストレイテンシは劣化しない。

## hot reload (server)

```
client                                    server (Arc<RwLock<Arc<Furigana>>>)
  |                                              |
  | POST /admin/reload (admin token)             |
  |--------------------------------------------->|
  |                                              |  spawn_blocking { build_furigana(paths) }
  |                                              |   →  let new = Arc::new(...)
  |                                              |   →  *state.furigana.write() = new
  |  {"status":"reloaded","dict_size":N} <-------|
  |                                              |
  | GET /furigana?... (一般 token)                |
  |--------------------------------------------->|
  |                                              |  let f = state.furigana.read().clone();
  |                                              |  // RwLock 解放
  |                                              |  process(f.as_ref(), &params)
  |  ...                                         |
```

`RwLock<Arc<Furigana>>` で reload 中も既存リクエストが使い切る前の Arc を握り続けられるため、ダウンタイムなし。

## 設計判断のメモ

- **decisions.md にしない**: ADR (Architecture Decision Records) は今のところ書くほどのスコープではないので、本書で軽くメモする方針
- **データ駆動 (TOML)**: ルール変更で再ビルド不要、PR が contributors からも入りやすい
- **Lindera + IPADIC 固定**: `embed-ipadic` で配布物に同梱。 Smart engine 上では band 50 の fallback として動作 (= 他 provider が一切覆わない位置だけで使われる)。 NEologd は opt-in feature flag で対応する案 (Phase 3 候補、[Issue #9](https://github.com/RyuuNeko1107/ja-furigana/issues/9))。 UniDic への runtime 切替は 2026-06 の A/B 評価 (corpus 802 件で IPADIC 100% vs UniDic 95.9%、 発音形化け + 短単位分解が要因) で見送り — UniDic は 0.2.0 の aType → bracket 注釈 offline 生成にのみ使う方針
- **discrete band lexicographic 比較**: 連続値 score の calibration 沼を回避。 band 値は 5 種類 (2000 / 1000 / 950 / 100 / 50) のみで、 各 layer の責務が明確
- **`Dict` の多重保持**: jukugo (≥2 字 default) / unihan (1 字 default) / rich (Entry with match) / kanji (`[[kanji]]` block) を別 HashMap で保持。 Smart engine の `DictBridgeProvider` が rich / kanji を walk して `MatchCondition` 評価する
- **`postprocess.toml`**: 辞書 / [[kanji]] block で表現しづらい文字列レベルの最終調整 (例: 「ジュウパー → ジュッパー」の促音化補正)。mode 別 (`hiragana` / `ruby` / `tts` / `romaji`) フィルタ + regex pattern + capture group 参照可
- **`Dict::from_toml_dir` 全階層再帰**: `core/works/<medium>/<title>.toml` のような任意深度のサブディレクトリを許容。配布 tar.gz の展開結果を想定するため symlink ループ対策は持たない (静的データ前提)。works/ の運用ルールは [`ja-furigana-dict/core/works/README.md`](https://github.com/RyuuNeko1107/ja-furigana-dict/blob/master/core/works/README.md) (公式読みのみ採録、出典コメント必須)
- **WASM は無し**: 一度実装したが `.wasm` が 57 MB と重いため削除。Web からは `furigana serve` (HTTP API) を推奨

## 関連リンク

- 公開 rustdoc: [docs.rs/ja-furigana](https://docs.rs/ja-furigana)
- contributors 向けの編集ガイド: [CONTRIBUTING.md](../CONTRIBUTING.md)
- ロードマップ: [ROADMAP.md](./ROADMAP.md)
