# ja-furigana lib (Rust)

Japanese furigana / TTS-prep engine。 Lindera + IPADIC + TOML データ駆動。

- **GitHub**: <https://github.com/RyuuNeko1107/ja-furigana>
- **crates.io**: `ja-furigana` (lib) + `ja-furigana-cli` (bin: `furigana`)
- **License**: MIT or Apache-2.0
- **MSRV**: Rust 1.89+

## 現 version + 進捗

- **LIVE**: `0.1.12` (2026-06-14、 0.1.0 cut は 2026-05-12)。 0.1.x patch は master HEAD から cut
  しており 0.2.0 開発分 (ScoringContext / ADR-0004 / bracket-accent parser / scoring pub(crate) 化) を
  含む実質 0.2.0-preview (semver は user 判断で 0.1.x 据え置き)。 直近 patch: 0.1.10 = **NameBoundaryPass**
  (人名+敬称の token 衝突補正、 ADR-0005 第 3 adapter)、 0.1.11 = **match_hits condition weighting**
  (match block の hit condition 数を literal=2/broad=1 で `Score::match_hits` に累積)、
  0.1.12 = **テスト品質刷新 + 実バグ修正一括** (CJK 拡張B〜H 漢字判定 [𠮷/𩸽 に furigana]、 IVS/異体字
  セレクタ除去、 漢数字 ≥100+助数詞 [三百回目→さんびゃくかいめ、 kansuji_to_arabic を百千万億 additive +
  〇 positional 一般化]、 romaji 無音脱落、 ruby 出力の区切り記号エスケープ、 matcher O(N²) 線形化、
  Mutex poison 回復、 reload preload、 enumeration 決定化。 mutation testing で corpus 非依存
  モジュールを網羅検証。 lib 491 + cli 48 test green、 corpus 1085/1085。 CHANGELOG 参照)。
  本番 furigana-api は wrapper `2.0.9` で lib 0.1.12 稼働 (2026-06-14 deploy、 health gate ✓)
- **`0.2.0` (master 実装中・未 release)**: accent **core は実装済** =
  bracket notation parse + `AccentResult`/`AccentToken` + CLI `--mode=accent`
  (`api.rs::to_accent` / `scoring/bracket.rs`、 ADR-0003)。
  **残件**: UniDic pitch accent (aType) 統合のみ (loanwords は alpha.21、
  numeric_phrases は 2026-06-11 に再統合済)。 cut 判断は user 主導 (PROPOSALS/intonation.md)

## alpha.10 task 進捗 (2026-05-11)

### ✅ completed (16)

- A 系 (foundation): A1 schema_version validator + A1b caller wire-up (load_rules_dir / Dict / Loanwords / SingleOverrides の file load 経路に必須化、 lib fixture も `[meta] schema_version = "2"` 化、 dict 側の coordinate stamp は E1 / alpha.11 dict release 側責任) / A2 dict format 拡張 / A3 matcher
- B 系 (Smart engine core): B1 Viterbi DP / B2 band lexicographic / B3 (b)(c) penalty / B4 Engine 切替
- C 系 (cross-cutting):
  - C1 保護トークン抽出 (`scoring/special.rs` の ProtectTokenProvider、 band 2000)
  - C2 アルファベット passthrough (`scoring/special.rs` の AlphabetPassthroughProvider、 hit band 1000 / miss band 100)
  - C3 数字 + 助数詞 / 大数スケール / SI 単位 / 日付 / 時刻 / 記号 / 素の数字 (`scoring/numbers.rs` の NumberCandidateProvider、 band 950) + Furigana::analyze 5 provider 構成に統合
  - C4 踊り字 (`scoring/odoriji.rs` の OdorijiProvider + apply_rendaku post-pass、 既存 Strict 連濁 logic は kana::voice_first_kana に共通化) + postprocess 独立性 doc
- D: bracket forward compat (lib strip)
- F1: scoring/analyze.rs standalone API + Furigana::analyze() / CLI --mode analyze / HTTP mode=analyze (alpha.10 段階で ProtectToken / AlphabetPassthrough / DictBridge / NumberCandidate / Odoriji の 5 provider 構成、 loanwords / numeric_phrases 統合は今後)
- F2: `furigana-diff-engines` CLI tool

### ⏳ pending (2)

- E1: migration script Python (**dict repo 側 work**)
- ~~H1: alpha.10 release prep~~ (撤回、 alpha.10 release 自体 skip 方針、 alpha.11+ 一括 release で対応)

## 主要 module 構造

```
crates/furigana/src/
├── api.rs                 — Furigana / FuriganaBuilder (公開 entry、 解析は scoring/pipeline 経由の薄い層)
├── analyzer.rs            — Lindera + IPADIC ラッパー
├── char_class.rs          — 文字種 (CharType) 分類 + Unicode range 表の single home (kana/matcher/special が参照)
├── dict.rs                — jukugo / unihan / rich entry / [[kanji]] block 多重保持 (先頭 char prefix index 付)
├── kana.rs                — ひら⇄カタ変換 + 連濁 (voice_first_kana)。 判定 3 関数は char_class への公開 delegate
├── loader.rs              — TOML loader (schema_version validate)
├── numbers/               — kansuji / 助数詞 logic (scoring/numbers.rs から呼ばれる)
├── reading/               — 出力 layer (ReadingToken + tokens_to_hiragana / tokens_to_ruby)
├── romaji.rs              — ひらがな → ローマ字 (Hepburn / Kunrei)
├── rules/                 — counters / context / days / scales / postprocess 等 TOML data
├── scoring/               — Smart engine module (詳細 別記)
└── tts.rs                 — TTS pre-processing (pause 整形 等)

(旧 chunks/ / loanwords.rs / single_overrides.rs / reading/pipeline.rs は alpha.15 で削除済。
 loanwords は alpha.21 で AlphabetPassthroughProvider に再統合。)

crates/furigana-cli/src/
├── main.rs                — `furigana` バイナリ (CLI + HTTP server)
├── commands/              — lookup / repl / serve / dict subcommands
└── bin/                   — dict_gap_mine 等 support tool
```

## scoring/ module (alpha.10 新設)

| sub module | 役割 |
|---|---|
| `pipeline.rs` ★ | **Pipeline facade** — 6 provider 構成 + Viterbi + Reading Post-pass を所有する single seam。 `tokens()` (production) / `analyze()` (debug)。 provider 追加・順序変更はここで完結 |
| `format.rs` | Entry / EntryDetail / MatchBlock / MatchCondition / KanjiBlock の struct (CharType は char_class.rs から re-export) |
| `matcher.rs` | MatchContext + matches_context() + pseudo-token 走査 + resolve_readings (classify_char は char_class.rs へ移動) |
| `candidate.rs` | Score / Candidate / CandidateProvider trait + ScoringContext + band 定数 |
| `engine.rs` | PathScore (weakest_band + edge_count agg) + solve_path Viterbi DP |
| `boundary.rs` | KanjiRegion + BoundaryAnalysis (b)(c) penalty -300/-600 |
| `special.rs` | ProtectTokenProvider (band 2000) + AlphabetPassthroughProvider (hit 1000 / miss 100、 loanwords lookup 込) |
| `dict_bridge.rs` ★ | DictBridgeProvider — Dict (jukugo / unihan / [[kanji]] block) の candidate 化、 先頭 char prefix index 引き |
| `numbers/` | NumberCandidateProvider (band 950: 助数詞 / 大数スケール / SI 単位 / 日付 / 時刻 / 記号 / 素の数字)。 `patterns.rs` = regex 定義+構築、 `mod.rs` = 候補種別ごとの try_* matcher |
| `odoriji.rs` | OdorijiProvider (々 placeholder) + RendakuPass (連濁 logic は kana::voice_first_kana 共通化) |
| `lindera_fallback.rs` | LinderaFallbackProvider (band 50/150 safety net + gap-passthrough) |
| `postpass.rs` | ReadingPostPass trait + apply_all 適用順 (ADR-0005) |
| `contextual.rs` | HaraSukuPass (腹+空く 2-token-back 補正) |
| `names.rs` | NameBoundaryPass (人名+敬称 token 衝突の再分割/merge、 読み source = dict→IPADIC 固有名詞) |
| `bracket.rs` | bracket notation parse → AccentPhrase (0.2.0 core) |
| `analyze.rs` | AnalyzeResult / Token + analyze() / analyze_tokens() (★11 freeze types) |
| `inspect.rs` | dict gap 抽出等の inspection helper (公開 re-export) |

## よく使うコマンド

```powershell
# build + test
cargo test --lib                             # 435 lib test (alpha.11 dict 完全再編成 完了時点)
cargo test --lib scoring::                   # scoring module のみ
cargo clippy --lib -- -D warnings            # clippy clean 確認
cargo fmt                                    # フォーマット

# CLI 動作確認
cargo run --bin furigana -- lookup "猫が好き" --mode hiragana

# corpus regression (高速一括、 Furigana 構築 1 回で全 corpus。 802 case ≈ 4 秒)
# ※ dict repo の tools/run_corpus.py (1 case ごと CLI 起動、 ~15 分) より常にこちらを使う
cargo run --release --bin furigana-corpus-check -- `
  --rules-dir ..\furigana-dict\rules --core-dict-dir ..\furigana-dict\core `
  ..\furigana-dict\tests\corpus
# UniDic 版: cargo build --release -p ja-furigana-cli --bin furigana-corpus-check --no-default-features --features dict-unidic

# benchmark
cargo bench --bench lookup                   # 代表入力の latency
cargo bench --bench scaling                  # 入力長スケーリング + alloc churn (実 dict は FURIGANA_BENCH_CORE/_RULES)
```

## 重要設計指針

- **Smart engine 一本化済** (alpha.15): 旧 Strict engine は削除済、 `Furigana::to_*` / `tokenize` / `analyze` はすべて `scoring/pipeline.rs` の Pipeline facade 経由 (= 同一の採択 path)
- **discrete band + lexicographic**: 連続値 score ではなく band/length/match_hits/penalty の 4 軸 lexicographic 比較 (= calibration 沼回避)
- **品詞 matcher 不採用**: Lindera 撤廃路線と整合、 `prev_pos` / `next_pos` は無し、 literal + char_type のみ
- **forward compat for intonation**: bracket notation `[` `]` `/` を 0.1.0 から dict 側で書ける、 lib は strip / 無視、 0.2.0 で活用

## 主要 doc

- `docs/PROPOSALS/scoring-engine.md` — 0.1.0 stable architecture 詳細
- `docs/PROPOSALS/intonation.md` — 0.2.0 stable target (Postponed → Planned for 0.2.0)
- `docs/ROADMAP.md` — phase + timeline
- `docs/ARCHITECTURE.md` — 既存 4 層構造
- `CHANGELOG.md` — 各 release 差分
- `CONTRIBUTING.md` / `MAINTAINING.md` — contributor / maintainer ガイド

## 注意点

- **branch protection 一時 OFF** (master): alpha.7 → alpha.8 経緯、 stable cut 前に復元
- **publish policy** (2026-05-11 再更新): **alpha 期間中は crates.io publish しない** (= 0.1.0 stable 再開)、 加えて **alpha.10 は GitHub release も skip** (= 4 commit は master push 済の内部 milestone label として残す)。 次の release は alpha.10 + alpha.11 work をまとめた alpha.11+。 既 publish 済 (`alpha.1` 〜 `alpha.9`) は metadata 不変のまま yank しない
- **dict version compat**: alpha.10 lib は `[meta] schema_version = "2"` のみ accept、 旧 format dict は parse error (= dict v2 化と coordinated)
