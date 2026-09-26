# ロードマップ

ja-furigana の中長期計画。 **完了履歴は [CHANGELOG.md](../CHANGELOG.md)** を参照。
本書は「これから何をやるか」 志向で書く。

> 戻る: [README](../README.md)

## ステータス概観 (2026-09-27 更新)

**最新 release は v0.5.0 (2026-09-24)**。 0.2.0 以降の主な節目 (詳細は CHANGELOG):

- **0.2.0 (2026-07-06)**: intonation milestone — accent core (bracket parse + `AccentResult` +
  `--mode=accent`)、 opt-in の rule-based accent 推定、 OOV 促音便 join、 VOICEVOX adapter crate
  `ja-furigana-voicevox`
- **0.3.0〜0.3.2 (2026-08-11〜09-09)**: 本家 AquesTalk adapter crate `ja-furigana-aquestalk` +
  共有コア `furigana::accent_symbols` + `TtsOptions::silence_symbols` (顔文字/絵文字の TTS silent 化)、
  AquesTalk の句数上限に合わせた分割 (0.3.1)、 英字語内ハイフン (Wi-Fi) 修正 (0.3.2)
- **0.4.0 (2026-09-11)**: 文スコープ match condition `input_contains_any`
- **0.4.1〜0.4.3 (2026-09-12〜15)**: 数値・単位・記号まわりの誤読修正、 ruby の空 group 修正、
  `furigana lookup --batch`
- **0.4.4 (2026-09-17)**: 性能改善 (dict bucket の先頭 2 文字区間引き等、 実文 約 1.7 倍速)
- **0.4.5〜0.4.7 (2026-09-18〜23)**: 誤読修正 (同字連続 / 1 字一段動詞 / 接尾辞の連濁形 /
  位取り漢数字 / 行き止まり補完)、 確保削減、 コスト lattice engine (opt-in 実験)
- **0.5.0 (2026-09-24)**: 助数詞 `not_before` / `scale_trailing` (破壊的: `rules::CounterRule` の
  `#[non_exhaustive]` 化)、 辞書保持の軽量化 (ピークメモリ 87 → 75 MB)

runtime 形態素辞書は IPADIC 据え置き (2026-06-11 A/B 評価で IPADIC 100% vs UniDic 95.9%、
詳細は [ARCHITECTURE.md](./ARCHITECTURE.md) 設計判断メモ)。 UniDic aType → bracket 注釈は
dict repo 側の offline 生成 tool で活用。

次の主なテーマは dict 側の継続改善と、 運用で発覚した lib 改善の随時 sweep。

0.x のため minor version で破壊的変更がありうる (patch は互換維持)。 Rust toolchain は
**1.89+** が必要 (`std::fs::File::lock` 安定化要求のため、 依存 rustyline 18 経由)。

## 完了済み

詳細は [CHANGELOG.md](../CHANGELOG.md) で。 サマリのみ:

### Phase 1〜2 (~2026-05-05)
- workspace + lib + CLI + データ駆動ルール (全 TOML)
- HTTP server (Axum)、 辞書管理コマンド、 GitHub Release ワークフロー
- `furigana-dict` リポジトリ開設 + seed 投入
- `furigana dict pull` (GitHub Releases + SHA-256 検証 + 展開)
- ホットリロード (`SIGHUP` / `POST /admin/reload`)
- portable 配置、 対話 REPL、 SI 単位 case-insensitive、 ローマ字出力モード
- crates.io 公開 (`ja-furigana` lib + `ja-furigana-cli` bin)
- Lindera analyzer の lazy init

### Phase 3 (~2026-05-06、 alpha.3)
- 本番互換の **5 段階優先順位** (`context rule → jukugo → Lindera → unihan`) (履歴: alpha.15 で Smart engine に置換)
- `Dict` の jukugo / unihan 内部分離
- `NumberChunker` の漢数字対応 + scale+unit 連結 + counter context
- `postprocess.toml` (Step 7 mode 別 regex 置換)
- 検証ループ駆動の品質改善基盤 (`should_read.toml` + `tools/run_corpus.py`)
- CI の audit / corpus regression job 追加

### Phase 4 (運用基盤)
- 辞書自動更新 (`--auto-pull` + `[auto_update]`、 admin_tokens 不要)
- `Dict::from_toml_dir` 全階層再帰
- 作品単位辞書 `core/works/` 新設
- 辞書大規模拡充 (jukugo 24 カテゴリ、 4.5k 件超)
- STATS.md 自動生成基盤

### Phase 5 (lookup priority + 外来語 + 出力ルール、 alpha.7)
- jukugo Aho-Corasick prefix-match (chunks 階層 4.5)
- 外来語 (loanwords) 辞書サポート (chunks 階層 4.7、 完全一致 lookup)
- 出力ルール仕様変更 (surface 文字種で reading 表記分岐)
- cross-file 重複検出の自動化 (validate.py + STATS_DUPS.md)
- 踊り字「々」 自動展開
- 単漢字 default override (`SingleOverrides`、 issue #15 限定解)

### Phase 6 (security + role 駆動 loader、 alpha.8〜alpha.9)
- security 全 8 軸補強 (archive 展開 caps / HTTP body limit + rate limit / ReDoS audit /
  RCE audit / sanitize layer for dict load / timing-safe token 比較 /
  GitHub tag strict format / CLI 制御文字 reject)
- `[meta] role` 駆動 loader (rules + dict 両方を統一 dispatch)
- rules 3 sub-dir 階層化 (numbers / context / output)
- inline test (`*.test.toml`) の append-only CI 強制
- dict TOML format DSL 化 (triple-quoted string)
- days.toml の `[entries]` block 化

## 進行中 / 候補

### 0.1.0 stable に向けて (履歴: 0.1.0 は 2026-05-12 に cut 済、 以下は当時の計画と実績)

#### Phase 7: Scoring Engine (candidate-based reading resolution) ★ 0.2.0
詳細仕様: [docs/PROPOSALS/scoring-engine.md](./PROPOSALS/scoring-engine.md)

「答えを持つ辞書 → 候補を出す辞書」 への architecture 転換。 ルビ振り精度向上が target、 韻律 / accent / TTS 連携は本 phase scope 外。

**dict 側 (ja-furigana-dict、 0.1.0 stable 同期)**

- [x] entry inline match notation (`[[entries."x".match]]` sub-table) 受け入れ (= alpha.11 機械変換完了)
- [x] `[[kanji]]` block first-class candidate generator (`core/kanji/` 新設) (= alpha.11、 seed 1 件 = 「土」)
- [x] `rules/context/` 廃止、 中身は entry inline に migration (= alpha.11、 51 surface)
- [x] `SingleOverrides` (Issue #15) を `[[kanji]]` block に統合 (= alpha.11、 single_overrides.toml 削除済)
- [x] migration script 実装 (= `tools/migrations/migrate_v2.py` + `migrate_v2_context.py` + `merge_migrated_context.py` + `migrate_kanji_format.py`)
- [x] reading に bracket notation (`[`, `]`, `/`) を許可 (= alpha.10、 forward compat for 0.2.0)
- [x] validate.py 拡張 (= schema_version + bracket syntax check 完了、 matcher vocabulary check は判定方針確定後)
- [x] **`docs/SCHEMA.md` 全面 update** (= alpha.11、 新 format 対応)
- [x] **`CONTRIBUTING.md` 更新** (= alpha.11、 detailed entry / bracket notation 入門追記)
- [x] **`docs/RECIPES.md` 新規** (= alpha.11、 「やりたいこと → 書き方」 cookbook)
- [ ] **重複 / 古い / 出典なし entry の purge** (人手 PR series、 alpha.12+ 漸進)
- [ ] **`core/jukugo/` 24 カテゴリ 再分類** (人手 PR series、 alpha.12+ 漸進、 5024 entry の review なので multi-week)
- [ ] **`core/works/` 作品単位 sub-dir 整理** (= 現状清潔、 必要時に拡充)
- [ ] **`core/loanwords/` 整理確認** (= 現状清潔、 必要時に拡充)
- [ ] **dict release pace Hybrid** (★15): SemVer (lib coordinated `v0.1.0` / `v0.2.0`) + CalVer (daily-release / 修正)、 daily-release.yml 再開は 0.1.0 cut 後 user 判断

**lib 側 (ja-furigana、 0.1.0 stable)**

- [x] candidate scoring engine 実装 (Viterbi-like path 選択、 **精度 + 効率最適化**)
- [x] discrete band + lexicographic 比較 (連続値 score 不採用)
- [x] entry inline match parser (untagged enum で省略形 / inline / expanded を吸収)
- [x] `[[kanji]]` block parser
- [x] matcher vocabulary 実装 (literal / char_type のみ、 **品詞 matcher は不採用**)
- [x] (b) 漢字連続 boundary penalty + (c) 未知語 chunk 強化 penalty
- [x] (a) longest match (length lexicographic 比較で実現)
- [x] **特殊処理 (cross-cutting) 再設計実装**: 保護トークン抽出 (URL/絵文字) / アルファベット passthrough / 数字 + 助数詞 (`NumberCandidateProvider`、 band 950) / 漢数字 / 数字読み / 踊り字 「々」 自動展開 / postprocess
- [x] **bracket notation forward compat**: 読み込み時に `[`, `]`, `/` を strip、 reading 部分のみ使用
- [x] ~~`Engine::Smart` / `Engine::Strict` の env var 切替~~ (obsolete: alpha.15 で Strict engine を削除し Smart engine に一本化、 切替機構も撤去)
- [x] `Furigana::analyze()` debug API (★11 確定型: AnalyzeResult / Token / Candidate / Score)
- [x] **CLI `--mode analyze` 追加** (analyze 出力 mode、 ★12)
- [x] **HTTP server schema freeze**: 既存 endpoint + `mode=analyze` 時に extra field (★13)
- [x] 旧 format は parse error (`[meta] schema_version` 検証で v2 要求、 ★5)
- [x] Lindera 形態素分割 + reading は継続使用 (`LinderaFallbackProvider`、 band 50 / 150、 品詞 matcher は使わない)
- [x] ~~`tools/diff_engines` (Smart vs Strict diff)~~ (obsolete: Strict engine 削除に伴い撤去)
- [x] **benchmark 整備** (criterion、 ★14): `benches/lookup.rs` / `scaling.rs` / `lindera_share.rs`、 目標と計測手順は [PERFORMANCE.md](./PERFORMANCE.md)
- [ ] **既存機能 freeze 確認 test** (★16): portable 配置 / REPL / SI 単位 / ホットリロード / `furigana dict pull`
- [x] CHANGELOG `[Unreleased]` 蓄積 → 0.1.0 cut 時 finalize (★17)
- [x] **MIGRATION.md 新規** (★17、 repo root の [MIGRATION.md](../MIGRATION.md))

**dict 側 contributor 規律 (`furigana-dict/CONTRIBUTING.md`)**

- [ ] 漢字 2 文字以下 entry の PR レビュー基準明文化 (= (e) 規律)
- [ ] 「○○魔館」 系 suffix 単独登録の禁止
- [ ] 出典明示と同等の重みで規律違反を merge block

#### Phase 8: 0.2.0 stable — intonation + 残 lib 改善 sweep ✅ (intonation 部は 0.2.0 で出荷済)

詳細仕様: [docs/PROPOSALS/intonation.md](./PROPOSALS/intonation.md) (Status: Shipped in 0.2.0)

**0.1.0 で建てた forward compat** (= bracket notation `[ ] /` strip 済 dict が大量に存在) を 0.2.0 で parse + 活用。 加えて 0.1.0 cut 後の運用で発覚した lib 改善 sweep を統合。

> **進捗 (2026-07-06、 v0.2.0 cut)**: intonation 一式は **release 済** — bracket parse +
> `AccentPhrase` / `--mode=accent`、 opt-in accent 推定 (ADR-0007)、 OOV 促音便 join
> (ADR-0008)、 adapter crate `ja-furigana-voicevox` の `--mode=voicevox-aques`
> (`voicevox-query` は不採用、 詳細は intonation.md 冒頭の ADR 差分注記)。
> `rules/accent/` / fractions は ADR-0002 で 0.2.0 scope から除外。
> UniDic aType は runtime 統合ではなく **offline bracket 生成 tool**
> (dict repo `tools/gen_accent_brackets.py`、 3,122 件適用済) に確定。
> 下記 「lib 改善 sweep」 のうち人名判定は NameBoundaryPass (ADR-0005) +
> accent 推定の人名判定で消化済。
>
> **追記 (2026-08-11): lib 改善 sweep は 4 項目すべて消化済**。
> ひらがな雑 match sweep = dict 側で完了 (`next_char_type = "ひらがな"` の実使用は 0 件、
> overrides.toml に 「使わない」 方針コメントのみ残存)、
> space normalize = 旧 `preprocess_input` hack を Lindera gap-passthrough 導入時に撤去済
> (半角 space 保持を test で固定)、
> 顔文字 TTS silent = `TtsOptions::silence_symbols` (CLI `--silence-symbols` /
> HTTP `silence_symbols`) として実装。

##### 主要 機能追加 (= intonation)

- **bracket notation parse 実装**、 `Token { accent_phrases }` field 追加 (additive、 `#[non_exhaustive]` で SemVer minor 互換)
- **`--mode=accent`** 中立 JSON 出力 (= engine 非依存の accent annotation)
- **`--mode=voicevox-aques`** AquesTalk-風記法
- ~~`--mode=voicevox-query` / `tts` mode の `include_accent`~~ — 不採用 (intonation.md 冒頭の注記参照)
- ~~`rules/accent/` 階層 + `rules/numbers/fractions.toml`~~ — 0.2.0 scope から除外 (dict bracket 由来の accent に一本化)
- **動的 accent shift rules** — 連濁 / 動詞活用 / 複合語 deaccenting / 助数詞 拡充 (未着手)

詳細は [intonation.md](./PROPOSALS/intonation.md) §0 / §8 参照。

##### 主要 lib 改善 sweep (= 0.1.0 運用で発覚、 2026-08-11 に 4 項目とも消化済 = 上記進捗参照)

- **`next_char_type = "ひらがな"` 雑指示の最小マッチ化 sweep** ([[kanji]] block 30+ 箇所)
  - 現状 「ひらがな全体マッチ」 で 想定外文脈で誤発火 (= 「復帰勢でも → フッキイキオデモ」 round 44 等の bug 温床)
  - `next_starts_any = ["い", "さ", "く"]` 等で必要 stem 文字を明示列挙して安全化
- **人名判定 lib logic** (= ipadic-name-bias)
  - 「○○ さん / 君 / 氏 / 様」 next 文脈で前 surface を 「人名 priority」 推定
  - 現状 personal_names.toml に 46 件主要姓 hardcoded、 lib 側 logic で自動化
- **顔文字 TTS skip / silent 化**
  - 「・」 → 「なかぐろ」、 「ω」 → 「おめが」 等の 1 字 phonetic 化が TTS で違和感
  - protect token / 顔文字 chunk を TTS 出力で **silent** にする option (= `--include-emoji-tts=false`)
- **半角 space normalize の正式化** (= 0.1.0 では `preprocess_input()` で 全角 space に変換、 0.2.0 で path 構築 logic に proper 統合)

##### corpus regression

回帰 corpus は [`ja-furigana-dict`](https://github.com/RyuuNeko1107/ja-furigana-dict) の
`tests/corpus/` (2026-09 時点で約 1.2 万 case、 release 時点で 100% pass)。
測定は `furigana-corpus-check` を使う (複数 corpus file / ディレクトリ一括対応、
Furigana 構築 1 回で全件):

```bash
cargo run --release --bin furigana-corpus-check --     --rules-dir <furigana-dict/rules> --core-dict-dir <furigana-dict/core>     <furigana-dict/tests/corpus>
```

> **注意**: dict repo の `tools/run_corpus.py` は 1 case ごとに CLI を起動するため
> 桁違いに遅い。 ローカルの regression 測定は常に `furigana-corpus-check` を使うこと。

##### 改善材料収集 (= 0.2.0 round 前準備)

- **VOICEVOX engine 一致率を主力指標として運用** (= 75-77% → 85% push)
  - 辞書 corpus 内部 expected/actual 一致率は dict 改善で 100% に飽和し dogfood 指標としては形骸化、
    一方 VOICEVOX engine kana 一致率は dict 改善が **実用 TTS 経路まで届くか** を測る本質指標
  - 旧 `compare_with_openjtalk.py` (= 単純 phonetic 一致度 lib 内部メトリック) は役目を終え 2026-05-12 削除済、
    現主力 dev tool は `data/_analysis/scripts/compare_with_voicevox.py` の単一窓口
  - 「セエ → セイ」 母音 / 拗音 phoneme / 句読点周辺の normalize pipeline 改良 (= dict 側ではなく 比較 tool 側) で残 diff を絞る
  - 過去案 「co-occurrence / word-pair stats dev tool」 は round 47 で normalize 強化 +
    dict 改善で verify avg 85% 達成、 残 diff は VV 側誤読 / lib bug が多く co-occurrence
    で抽出できる dict candidate は marginal、 不採用

#### 0.1.0 cut 後 TODO (= 1〜3 ヶ月運用後判断)

- [x] **大規模 QA corpus 増強** — 回帰 corpus は約 1.2 万 case まで拡充済 (継続)
- [ ] **user_dict CSV 化検討** — 同形異音語 misclassification / 複合語 boundary ずれが
      頻発するなら、 user 側 dict 拡張 API 追加
- [ ] **accent bracket の拡充** — dict 側で接頭辞 / 接尾辞 / 各 counter の bracket を地道に蓄積
- [ ] **NHK アクセント新辞典 出典の bulk PR** — 出典 license 確認後、 まとまった量の seed PR
- [ ] **engine adapter の community 受付** — openjtalk / ssml / ymm4 等は community PR 待ち、
  必要なら engine config 外部 TOML 化アーキテクチャを検討

#### timeline 実績 (2026-05-12 更新)

**0.1.0 cut 完了 (2026-05-12)**:
- alpha.10〜.12: scoring-engine 投入 + dict format 拡張 + [[kanji]] block loader
- alpha.13: Lindera fallback provider + Smart engine が corpus で実用域へ (82% match)
- alpha.14: Smart engine を `to_*` API に wire-up (= production path)
- alpha.15: Strict engine 完全削除 (-3000 行)、 Smart engine 一本化
- alpha.16〜.17: dict 拡充 + UniDic feature flag (`dict-unidic`)
- alpha.18: ↓ (alpha.19 で撤回されたが lib band hack 試行)
- alpha.19: dict-curated context rule 路線統一 (= 動詞 / 形容詞 1 字 [[kanji]] block 化) + inspect API
- alpha.20: 形態素信頼 band-up (= `BAND_LINDERA_COMPOUND = 150`、 dict 未登録の純漢字熟語救済)
- alpha.21: dict 改善 round 31-46 (= 動詞訓読み default 偏向 sweep 60+ 字) + 公開 API wrapper (= signal_log) + lib 半角 space bug fix + tower_governor ConnectInfo fix
- **v0.1.0 stable cut**: 主要 corpus (当時 262 case) 99.2% / OpenJTalk 83-85% / VOICEVOX 75-77% / crates.io publish 再開 / dict v0.1.0 coordinated

**今後 (0.1.x → 0.2.0)** (履歴: 0.2.0 は 2026-07-06 に cut 済、 以降は冒頭のステータス概観参照):
- **0.1.x patch**: dict 漸進拡充 / corpus 増強 / bug fix (additive only)、 daily-release 自動 cut 運用
- **0.2.0 stable**: 上記 intonation + lib 改善 sweep
- **0.3.0+**: UniDic csj (= 現代話し言葉)、 連濁 / 動詞活用 accent shift、 lindera-neologd opt-in

0.2.0 までの実時間見積もり: **半年〜1 年規模** (= intonation の dict 蓄積 + lib sweep 規模)、 期日 driven ではなく完成度優先。

**0.2.0 stable の position**: 「intonation / accent annotation が**確実に動く**段階」 + 「0.1.0 で発覚した lib bug temperance」。 [intonation.md](./PROPOSALS/intonation.md) §0 が大方針、 残 lib 改善は 0.1.x patch で漸進対応可能なら 0.2.0 を待たずに先行 release 検討。

### 0.2.0+ 並走候補 (= 0.1.x patch 〜 0.2.0 にかけて漸進、 release blocker ではない)

- [ ] **作品単位辞書の継続拡充** — `core/works/` 構造に他作品を PR ベースで追加、
  サブポリシー (公式読みのみ採録 + 出典 comment 必須) を満たすもの
- [ ] **`lindera-neologd` opt-in feature flag** ([Issue #9](https://github.com/RyuuNeko1107/ja-furigana/issues/9))
  - 新語 / 商標 / アニメ作品名等が default で読めるようになる
  - 一方で binary 肥大化 (~50 MB → 数百 MB)、 NEologd は upstream 凍結中、
    過剰な複合語化の懸念
  - feature flag で choice にする案
- [ ] **辞書ピンの依存表記** — `Cargo.toml` 経由で辞書 version を declare できるように?
  - `cargo install ja-furigana-cli --features dict-pinned` のような切り口
- [ ] **postprocess ルールの拡充** — 土台 (mode 別 regex) はあるが具体ルールは少数。
  汎用的に使える rule を蓄積する
- [ ] **検証バッチからの corpus promote** — `tools/verify_batch.txt` で見つけた
  empirical な誤読修正を `ja-furigana-dict/tests/corpus/should_read.toml` に
  promote して回帰検証に組み込む

## 長期 vision (1.0+)

### 形態素解析依存の段階的撤廃

現在は `Lindera + IPADIC` で tokenize し、 `ja-furigana-dict` で override する 4 層構造。
dict 規模が 50k → 200k → 500k と育つにつれ、 Lindera が貢献する文脈が逓減する。

```
将来 vision (0.3.x or 1.0+):
入力 → ja-furigana-dict (longest-match + 活用 rule + 助詞 boundary) → 出力
        ↑ pure Rust、 deps 最小、 軽量、 license clean
```

便益:
- pure Rust deps 最小化
- 配布物軽量化 (Lindera + IPADIC 同梱で binary 数 MB)
- license obligation 減 (Lindera は MIT、 IPADIC は BSD だが、 撤廃で完全 control)

必要条件:
- dict が 200k+ entries (現在 50k)
- 動詞活用 rule layer
- 助詞 boundary detector
- 形態素解析無しで品質保てる corpus regression

0.1.0 stable で固める accent annotation の流儀 (TOML bracket、 user_dict CSV 不採用)
は、 この方向と整合する: accent は dict TOML、 形態素解析の中ではない。

## 廃止された候補

過去に検討したが、 別アプローチで代替したもの:

- ❌ **WebAssembly ビルド** — `.wasm` が Lindera + IPADIC 込みで 57 MB と重く、
  ブラウザから直接ロードするには不向きだった。 Web からは `furigana serve` (HTTP API)
  で十分という判断で削除 (alpha.4)
- ❌ **本体バイナリへの辞書 embed** — バイナリ肥大化 / 利用者ごとの辞書差し替え不能 /
  PR ループの遅さで却下。 `furigana-dict` 別 repo + `furigana dict pull` の構成に

## ロードマップ更新ポリシー

- 完了したものは [CHANGELOG.md](../CHANGELOG.md) `[Unreleased]` に移し、 本書からは
  サマリ 1 行に圧縮
- 大きい設計判断は本書ではなく [ARCHITECTURE.md](./ARCHITECTURE.md) に書く
