# ja-furigana lib (Rust)

Japanese furigana / TTS-prep engine。 Lindera + IPADIC + TOML データ駆動。

- **GitHub**: <https://github.com/RyuuNeko1107/ja-furigana>
- **crates.io**: `ja-furigana` (lib) + `ja-furigana-cli` (bin: `furigana`) +
  `ja-furigana-voicevox` / `ja-furigana-aquestalk` (TTS 記号列 adapter)
- **License**: MIT
- **MSRV**: Rust 1.89+

## 現 version + 進捗

- **LIVE**: `0.5.0` (2026-09-24、 crates.io 4 crate publish + tag v0.5.0)。 内容 =
  **助数詞 `not_before` / `scale_trailing`** (助数詞の字が動詞語幹を兼ねる時の衝突よけ、
  4000行って → 行く の活用) + **辞書保持の軽量化** (jukugo map 廃止、 ピークメモリ 1,000 行 87 → 75 MB)。
  破壊的変更は `rules::CounterRule` の `#[non_exhaustive]` 化 1 点。
- **master (未 release)**: 日付の漢数字の位取り (一二月 = 12 月) / 数字と助数詞の間の半角空白 +
  分数 N分のM (CHANGELOG `[Unreleased]`)
- (履歴) `0.4.5`〜`0.4.7` (2026-09-18〜23): 誤読 fix (同字連続 / 1 字一段動詞 / 接尾辞の連濁形 /
  位取り漢数字 / 行き止まり補完) + 確保 35〜43% 削減 + cost lattice engine (opt-in 実験、
  `FURIGANA_COST_ENGINE=1`、 既定は band engine)。 詳細は CHANGELOG
- (履歴) `0.4.4` (2026-09-17、 crates.io 4 crate publish + GitHub release v0.4.4 =
  5 platform binary + Docker)。 内容 = **性能改善 3 件** (実文 約 1.7 倍速: long 287→200µs /
  medium 85→52µs) + 促音化 fix。 破壊的変更なし、 **公開 API も出力も不変** (corpus 11,058 件 100%)。
  最大の効き所は `DictBridgeProvider` の bucket **全件走査** 除去 (= sort 済み bucket に
  `partition_point` 2 回で先頭 2 文字の連続区間を取る `Dict::rich_matching_prefix`)。
  「御」 713 件 / 「大」 358 件 のような **実文で頻出する字だけ bucket が肥大** しており、
  辞書改善を続けるほど遅くなる構造だった。
  残件: 入力正規化の確保削減 (`normalize_char_piece`) は **速度未測定** (計測機が OBS 常駐で
  同一バイナリの連続実行ですら ±50% 振れたため)、 静かな環境で `cargo bench --bench lookup` 再計測のこと
- (履歴) `0.3.1` (2026-08-12、 crates.io 4 crate publish = ja-furigana / ja-furigana-voicevox /
  **ja-furigana-aquestalk** (新規) / ja-furigana-cli、 tag v0.3.0 + v0.3.1)。
  0.3.0 = TTS adapter 2 本立て (VOICEVOX kana 記法 + 本家 AquesTalk 音声記号列) + 共有コアの
  lib 移設 (`furigana::accent_symbols`、 ADR-0009) + `TtsOptions::silence_symbols`
  (顔文字/絵文字の TTS silent 化)。 破壊的変更は `TtsOptions` の `#[non_exhaustive]` 化のみ
  (`with_*` setter を追加)。 0.3.1 = AquesTalk 実機検証で判明した分割バグ修正
  (上限は文字数でなく 「`。` を挟まず並ぶ句が 27 まで」、 `MAX_PHRASES` 追加)。
- (履歴) `0.2.0` (2026-07-06 stable cut、 crates.io 3 crate publish = ja-furigana /
  ja-furigana-voicevox / ja-furigana-cli)。 0.2.0 = intonation milestone 完了:
  accent core (bracket parse + `AccentResult` + `--mode=accent`) + opt-in accent 推定
  (ADR-0007) + OOV 促音便 join (ADR-0008) + VOICEVOX adapter crate (ADR-0001) +
  serve 配線 (`--estimate-accent` / `mode=voicevox-aques`)。 0.1.17 からの破壊的変更なし。
- (履歴) `0.1.16` (2026-06-14、 0.1.0 cut は 2026-05-12)。 0.1.x patch は master HEAD から cut
  しており 0.2.0 開発分 (ScoringContext / ADR-0004 / bracket-accent parser / scoring pub(crate) 化) を
  含む実質 0.2.0-preview (semver は user 判断で 0.1.x 据え置き)。 直近 patch: 0.1.10 = **NameBoundaryPass**
  (人名+敬称の token 衝突補正、 ADR-0005 第 3 adapter)、 0.1.11 = **match_hits condition weighting**
  (match block の hit condition 数を literal=2/broad=1 で `Score::match_hits` に累積)、
  0.1.12 = **テスト品質刷新 + 実バグ修正一括** (CJK 拡張B〜H 漢字判定 [𠮷/𩸽 に furigana]、 IVS/異体字
  セレクタ除去、 漢数字 ≥100+助数詞 [三百回目→さんびゃくかいめ、 kansuji_to_arabic を百千万億 additive +
  〇 positional 一般化]、 romaji 無音脱落、 ruby 出力の区切り記号エスケープ、 matcher O(N²) 線形化、
  Mutex poison 回復、 reload preload、 enumeration 決定化。 mutation testing で corpus 非依存
  モジュールを網羅検証。 lib 491 + cli 48 test green、 corpus 1085/1085。 CHANGELOG 参照)。
  0.1.13 = **入力正規化を「lookup のみ正規化・表示 surface 原文保持」で配線** (compat/IVS/NFKC が
  alpha.15 以降 production 未配線だった regression。 `kana::normalize_text_aligned` で char 単位
  alignment を保持し解析後に surface/range を原文へ remap = `髙田→{髙田|たかだ}` / `３本→{３本|さんぼん}`、
  通常入力は fast path で挙動不変)。 0.1.14 = **compat を core_dict_dir からも読み込む**
  (`FuriganaBuilder` が rules_dir に加え core/user dict の role="compat" も補完、 defense-in-depth)。
  ※production の真の compat 修正は **dict 側で `compat.toml` を core/ → rules/ へ移動** (dict v2026.06.14、
  `load_rules_dir` が rules_dir 走査で拾うのでどの lib version でも有効)。
  0.1.15 = **複数文字 canonical の compat 完全展開** (旧字漢数字 `廿→二十` / `卅→三十` 等 5件、
  従来は先頭1文字で `廿→二`)、 0.1.16 = **多文字展開が token 分割された際の空 surface ruby 修正**
  (`卅→{卅|さん}{|じゅう}` を、 空 surface token の読みを直前 token に結合して `{卅|さんじゅう}` に。
  0.1.13 surface 保持機構の latent bug)。 lib 499 + cli 48 test green。
  既知の限界: `卅日` 等 旧字漢数字+助数詞は誤読しうる (展開後 十日=とおか と誤分割、 稀ケース)。
  (dict v2026.06.14 と組み合わせて 髙田→たかだ / 廿→にじゅう / 卅→さんじゅう / ３本→さんぼん を確認)
- UniDic aType は runtime 統合ではなく **offline bracket 生成 tool** に確定
  (dict repo `tools/gen_accent_brackets.py`、 core/jukugo に bracket 3,122 件適用済 =
  dict v2026.07.04)。 runtime 形態素辞書は IPADIC 据え置き (ADR-0006)

## 履歴メモ

- alpha.10 (2026-05) で Smart engine (Viterbi DP + band lexicographic + 6 provider) を新設し、
  alpha.15 で旧 Strict engine / chunks を削除して一本化。 詳細は CHANGELOG の alpha 各節

## 主要 module 構造

```
crates/furigana/src/
├── accent_symbols.rs      — TTS 記号列 adapter の共有コア (AccentResult → MoraPhrase + PhraseBreak)
├── api.rs                 — Furigana / FuriganaBuilder (公開 entry、 解析は scoring/pipeline 経由の薄い層)
├── analyzer.rs            — Lindera + IPADIC ラッパー
├── char_class.rs          — 文字種 (CharType) 分類 + Unicode range 表の single home (kana/matcher/special が参照)
├── dict.rs                — unihan / rich entry / [[kanji]] block 保持。 rich は sort 済み bucket を
│                            先頭 2 文字で区間引き (`Dict::rich_matching_prefix`)
├── embedded.rs            — 埋め込みデータ (本体には rules を embed しない = 空 RulesData)
├── error.rs               — FuriganaError / Result
├── kana.rs                — ひら⇄カタ変換 + 連濁 (voice_first_kana)。 判定 3 関数は char_class への公開 delegate
├── loader.rs              — TOML loader (schema_version validate)
├── numbers/               — kansuji / 助数詞 logic (scoring/numbers/ から呼ばれる)
├── reading/               — 出力 layer (ReadingToken + tokens_to_hiragana / tokens_to_ruby)
├── romaji.rs              — ひらがな → ローマ字 (Hepburn / Kunrei)
├── rules/                 — counters / days / scales / units / symbols / numeric_phrases / compat / postprocess の TOML schema
├── sanitize.rs            — 辞書 value の sanitize (制御文字 / bidi override / 過大長を load 時に reject)
├── scoring/               — Smart engine module (詳細 別記)
└── tts.rs                 — TTS pre-processing (pause 整形 等)

(旧 chunks/ / loanwords.rs / single_overrides.rs / reading/pipeline.rs は alpha.15 で削除済。
 loanwords は alpha.21 で AlphabetPassthroughProvider に再統合。)

crates/furigana-voicevox/    — VOICEVOX kana 記法 adapter (ADR-0001)
crates/furigana-aquestalk/   — 本家 AquesTalk 音声記号列 adapter (ADR-0001、 Converter facade 付)

crates/furigana-cli/src/
├── main.rs                — `furigana` バイナリ (CLI + HTTP server)
├── commands/              — lookup / repl / serve (auth / handlers / metrics / types) / dict subcommands
└── bin/                   — support tool (furigana-corpus-check / furigana-analyze-one / furigana-dict-gap-mine)
```

## scoring/ module

| sub module | 役割 |
|---|---|
| `pipeline.rs` ★ | **Pipeline facade** — 6 provider 構成 + Viterbi + Reading Post-pass を所有する single seam。 `tokens()` (production) / `analyze()` (debug)。 provider 追加・順序変更はここで完結 |
| `format.rs` | Entry / EntryDetail / MatchBlock / MatchCondition / KanjiBlock の struct (CharType は char_class.rs から re-export) |
| `matcher.rs` | MatchContext + matches_context() + pseudo-token 走査 + resolve_readings (classify_char は char_class.rs へ移動) |
| `candidate.rs` | Score / Candidate / CandidateProvider trait + ScoringContext + band 定数 |
| `engine.rs` | PathScore (weakest_band → edge_count → total_match_hits → synthetic_edges の lexicographic) + solve_path Viterbi DP (行き止まり補完込み) |
| `boundary.rs` | KanjiRegion + BoundaryAnalysis (b)(c) penalty -300/-600 |
| `special.rs` | ProtectTokenProvider (band 2000) + AlphabetPassthroughProvider (hit 1000 / miss 100、 loanwords lookup 込) |
| `dict_bridge.rs` ★ | DictBridgeProvider — Dict (rich entry / unihan / [[kanji]] block) の candidate 化、 先頭 2 文字の区間引き |
| `numbers/` | NumberCandidateProvider (band 950: 助数詞 / 大数スケール / SI 単位 / 日付 / 時刻 / 記号 / 素の数字)。 `patterns.rs` = regex 定義+構築、 `mod.rs` = 候補種別ごとの try_* matcher |
| `odoriji.rs` | OdorijiProvider (々 placeholder) + RendakuPass (連濁 logic は kana::voice_first_kana 共通化) |
| `lindera_fallback.rs` | LinderaFallbackProvider (band 50/150 safety net + gap-passthrough) |
| `postpass.rs` | ReadingPostPass trait + apply_all 適用順 (ADR-0005) |
| `contextual.rs` | HaraSukuPass (腹+空く 2-token-back 補正) |
| `phonojoin.rs` | SokuonJoinPass (OOV 漢字複合語の促音便 join、 ADR-0008) |
| `accent_estimate.rs` | rule-based accent 推定 (opt-in `estimate_accent`、 ADR-0007) |
| `lattice.rs` | コスト lattice engine (opt-in 実験、 `cost_engine` / `FURIGANA_COST_ENGINE=1`、 ADR-0011) |
| `names.rs` | NameBoundaryPass (人名+敬称 token 衝突の再分割/merge、 読み source = dict→IPADIC 固有名詞) |
| `bracket.rs` | bracket notation parse → AccentPhrase (0.2.0 core) |
| `analyze.rs` | AnalyzeResult / Token + analyze() / analyze_tokens() (★11 freeze types) |
| `inspect.rs` | dict gap 抽出等の inspection helper (公開 re-export) |

## よく使うコマンド

```powershell
# build + test
cargo test --lib                             # 約 630 lib test (0.5.0 後の master)
cargo test --lib scoring::                   # scoring module のみ
cargo clippy --lib -- -D warnings            # clippy clean 確認
cargo fmt                                    # フォーマット

# CLI 動作確認
cargo run --bin furigana -- lookup "猫が好き" --mode hiragana

# corpus regression (高速一括、 Furigana 構築 1 回で全 corpus。 約 1.2 万 case)
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
- **discrete band + lexicographic**: 連続値 score ではなく PathScore の 4 軸 (weakest_band → edge_count → total_match_hits → synthetic_edges) lexicographic 比較 (= calibration 沼回避)
- **品詞 matcher 不採用**: Lindera 撤廃路線と整合、 `prev_pos` / `next_pos` は無し、 literal + char_type のみ
- **forward compat for intonation**: bracket notation `[` `]` `/` を 0.1.0 から dict 側で書ける、 lib は strip / 無視、 0.2.0 で活用

## 主要 doc

- `docs/PROPOSALS/scoring-engine.md` — 0.1.0 stable architecture 詳細
- `docs/PROPOSALS/intonation.md` — intonation 仕様 (0.2.0 で出荷済)
- `docs/ROADMAP.md` — phase + timeline
- `docs/ARCHITECTURE.md` — crate / module 構成と engine 設計
- `CHANGELOG.md` — 各 release 差分
- `CONTRIBUTING.md` / `MAINTAINING.md` — contributor / maintainer ガイド

## 注意点

- **branch protection ON** (master): required status checks = Lint / Test (ubuntu/windows) /
  Security audit / License audit / Analyze (rust)=CodeQL / Corpus regression /
  Diff coverage (llvm-cov+diff-cover) / Mutation (changed lines)。strict=true、
  enforce_admins=false (オーナーは unsigned で直 push 可、 既存履歴も admin bypass)。
  テスト要件フレームワーク (`../テスト要件/`) の CI ゲートを 2026-06-17 に追加
- **publish policy**: 0.1.0 stable 以降は release ごとに crates.io publish (4 crate、 順序は
  lib → voicevox / aquestalk → cli、 手順は MAINTAINING.md)。 既 publish 済 alpha (`alpha.1` 〜 `alpha.9`) は yank しない
- **dict version compat**: lib は `[meta] schema_version = "2"` のみ accept、 旧 format dict は parse error (= dict v2 化と coordinated)
