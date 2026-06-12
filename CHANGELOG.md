# Changelog

このプロジェクト (`ja-furigana` lib + `ja-furigana-cli` bin) の変更履歴。
[Keep a Changelog](https://keepachangelog.com/ja/1.1.0/) 形式に概ね従い、
バージョニングは [Semantic Versioning](https://semver.org/lang/ja/) を採用。

## [Unreleased]

### Added

- **`NameBoundaryPass` (人名+敬称の token 衝突補正、`scoring/names.rs`)**: 「白上さん」 が
  白 | 上さん (= IPADIC 「上さん (カミサン)」)、 「戌神様」 が 戌 | 神様 のように、
  名前末尾 + 敬称が 1 語として形態素辞書に存在すると Viterbi path が名前境界を割る
  問題を、 ADR-0005 の Reading Post-pass 第 3 adapter として補正。 合成 suffix 形
  (白 | 上さん) と bare suffix 形 (白 | 上 | 氏) の両方に対応し、 結合 surface を
  dict 熟語 (works 含む) → IPADIC 単一 token + 固有名詞 の順で照会、 hit した場合のみ
  再分割 / merge する (普通名詞は対象外 = 誤爆防止)。 916k 実コーパス A/B で副作用 1 件
  (きたしん→ほくしん、 固有名詞標準読みへの変化) のみ確認済。 単漢字の名前読み切替
  (舞さん=まい 等) は dict `[[kanji]]` block の人名 suffix match が担当 (data 側) で、
  本 pass は境界修復のみを担う

### Changed

- **`ReadingPostPass::apply` の引数を `&mut Vec<Token>` に変更** (旧 `&mut [Token]`):
  `NameBoundaryPass` の bare suffix 形 merge が token 数を減らすため。 scoring module は
  pub(crate) のため公開 API への影響なし

## [0.1.9] - 2026-06-11

漢数字 + 助数詞の bare 読みを dict 制御で有効化 (data 駆動の opt-in 機構)。

### Added

- **`CounterRule.kanji_numeral` フラグ** (`rules/counters.rs`): 漢数字 (五/七/十) +
  助数詞を bare で counter 化するかを助数詞ごとに opt-in できる。従来は recursive 形
  (「一個目」) のみ counter 化し bare 形 (「五匹」) は Lindera 委譲だった (「一日中」 の
  「一日」 誤 counter 化を避けるため)。`build_counter_regexes` の漢数字 regex で recursive
  group を optional 化し、bare match の採否を `try_counter_kanji` が `kanji_numeral` で
  gate する。euphony は既存 `kansuji_to_arabic` + `euphonic_counter_read` が担うので連濁
  (三羽→さんば) / 促音 (六匹→ろっぴき) も自動。どの助数詞を opt-in するかは dict
  (`rules/numbers/counters/*.toml`) が制御 (= 辞書が source of truth)。`KANJI_NUM_PAT`
  guard により日本 / 立派 / 乾杯 等の非 counter 衝突なし。serde 未知フィールド無視で
  旧 dict 互換。

## [0.1.8] - 2026-06-11

数詞慣用語句 (二十歳=ハタチ 等) の復活 + Smart engine 内部の facade 化 + dev tool 拡充。

### Added

- **`numeric_phrases` を Smart engine に再統合** (`scoring/numbers/`): 旧 Strict engine
  撤廃 (alpha.15) 以降 data だけ残って未使用だった数詞慣用語句 (`二十歳=ハタチ` /
  `明後日=アサッテ` / `十分間=ジュップンカン` 等) を `NumberCandidateProvider` の
  candidate (band 950) として復活。 「明後日」 のような数字以外の先頭を持つ語句も
  拾えるよう、 数字先頭 fast-path 判定より前に評価する。 dict 完全一致 (band 1000)
  での個別 override は引き続き可能。 0.2.0 残件の 1 つを消化 (残りは UniDic aType)
- **`benches/scaling.rs` 追加**: 入力長スケーリング (1x/4x/16x) + 漢字連続 run +
  counting allocator による alloc churn レポート。 性能 regression detector 用
  (実 dict は `FURIGANA_BENCH_CORE` / `_RULES` で mount)
- **`furigana-corpus-check` の複数 corpus / ディレクトリ一括対応**: dir 指定で再帰
  `*.toml` 収集、 Furigana 構築 1 回で全 corpus を流す (802 case ≈ 4 秒)。
  summary に morph dict 種別 (ipadic / unidic) を表示し A/B 比較 log を区別可能に

### Changed

- **Smart engine パイプラインを `scoring/pipeline.rs` facade に集約** (内部 refactor、
  挙動変更なし): provider 構成 + Viterbi + Reading Post-pass の所有を 1 module 化、
  `DictBridgeProvider` は `scoring/dict_bridge.rs` へ移動、 `api.rs` は薄い公開層に。
  `scoring/numbers.rs` は `patterns.rs` (regex 構築) と候補種別ごとの `try_*` matcher
  に内部分割。 alpha.15 で削除済の旧 module (chunks / reading::pipeline / loanwords /
  single_overrides) への陳腐化 doc 参照も一掃

## [0.1.7] - 2026-06-10

半角スペースの出力保持 + HTTP server のリクエストタイムアウト追加。

### Changed

- **半角スペース / タブを出力にそのまま保持** (`api.rs`): 旧実装は `to_ruby` /
  `to_hiragana` / `to_tts` の前段 `preprocess_input` で半角 space / tab を全角
  space (U+3000) に置換していた (= scoring engine が ASCII whitespace を candidate
  化できない問題の workaround)。 0.1.6 の Lindera fallback gap-passthrough により
  落とされた空白も passthrough edge で覆えるようになったため、 この hack は不要に
  なり撤去。 半角 space は全角化けせず往復するようになった (例: `"hello world"` が
  `"hello　world"` にならない)。 英字混在テキストの TTS / 表示で自然な挙動。

### Security

- **`furigana serve` に 30 秒のリクエストタイムアウトを追加** (`commands/serve/mod.rs`):
  `tower_http::timeout::TimeoutLayer` で 1 リクエストの上限処理時間を設定。 変換は
  通常 sub-ms〜数 ms なので、 これを超える異常リクエストは 408 で打ち切り、 ワーカーの
  占有を防ぐ (defense-in-depth)。 body サイズ上限 (1 MB) / rate limit と合わせた多層防御。

## [0.1.6] - 2026-06-10

改行を含む input で出力が空になる bug の修正 (公開 API 非破壊)。

### Fixed

- **改行 / 制御文字を含む input が全空出力になる bug を修正** (`scoring/lindera_fallback.rs`):
  Lindera は `\n` / `\r` 等を token から落とすため byte offset がズレ、 従来は
  safety net の `LinderaFallbackProvider` が input 全体の edge を全廃して
  自身を無効化していた。 結果、 改行位置と周辺の助詞が誰にも覆われず Viterbi
  path が構築不能 (`dp[n]` 未到達) になり、 `to_ruby` / `to_hiragana` / `to_tts` /
  `to_romaji` が空文字列を返していた (例: 複数段落テキスト)。 Lindera が落とした
  空白 / 制御文字区間を前方探索でアラインし直し、 reading = surface の
  passthrough edge で補うように変更。 改行は出力にそのまま保持される。

## [0.1.5] - 2026-06-10

辞書 lookup hot path の大幅高速化 (挙動不変、 公開 API 非破壊)。 実 dict (47k entry)
での `to_ruby` latency を long 文で **約 200x** 短縮 (38 ms → 0.19 ms)、 短文は
1.8 ms → 8 µs。 corpus 回帰 801 件 / lib test 429 件 pass で挙動同一を確認。

### Performance (internal)

- **dict prefix scan を O(N×M) → O(N×k) 化** (`dict.rs` / `api.rs`): `DictBridgeProvider`
  が各 byte 位置で全 ~44k entry (`rich_iter`) と全 ~2.7k `[[kanji]]` block を linear
  prefix scan していた。 surface 先頭 char で逆引きする lazy index (`rich_index` /
  `kanji_index`、 `OnceLock`、 insert/merge で invalidate) を導入し、 各位置で
  「その文字で始まる entry だけ」 を走査するように。 位置毎の `HashSet<String>`
  dedup も撤去 (= 判定対象は常に先頭 1 字 surface なので bool で等価)。
- **数値 provider に先頭文字 dispatch を追加** (`scoring/numbers.rs`):
  `NumberCandidateProvider` が全位置で 6+ 正規表現を試行していた。 先頭が数字系
  (digit / 全角 / 漢数字 / 符号) でなければ数値系 regex は必ず空振りするため、
  記号 section だけ評価して即 return。 dict scan 撤去後の relative hot path を解消。
- **production 経路の debug 集約を分離** (`api.rs` / `scoring/analyze.rs`): `to_ruby`
  等は reading しか使わないのに、 `analyze()` が debug 用 `candidates` 再収集
  (全 provider を path 位置で再走) と `alternatives` 抽出を毎回行っていた。 軽量
  `analyze_tokens` 経路を新設して production の `to_*` / `tokenize` をそちらに。
  inspect 用 public `analyze()` は full のまま維持。
- **provider 構成を `with_analysis` に集約** (`api.rs`): `analyze` / `analyze_tokens`
  に重複コピペされていた 6 provider + boundary + ctx 構築を 1 箇所に (= provider
  追加時の drift 源を除去)。 stale だった loanwords doc も更新。
- **bench に実 dict 計測 hook を追加** (`benches/lookup.rs`): `FURIGANA_BENCH_CORE`
  / `FURIGANA_BENCH_RULES` 環境変数で実 dict を mount できるように (= seed 12 entry
  では dict 規模依存の bottleneck が顕在化しない)。 dict load 総時間の `build` group も追加。

## [0.1.4] - 2026-06-07

scoring engine の内部リファクタリング (挙動不変、 `scoring` は `pub(crate)` で
SemVer 非破壊)。 アーキテクチャ・レビューで挙がった deepening を反映。

### Changed (internal)

- **死配線だった boundary penalty を撤去** (`scoring/boundary.rs` / `candidate.rs`
  / `engine.rs`): (b)(c) penalty (-300/-600) は `penalty_at()` が `#[cfg(test)]` で
  本番未配線、 `Score.boundary_penalty` は常に 0 の死軸だった。 過分割抑制は
  `PathScore.edge_count` で足りるため penalty 経路を撤去し Score / PathScore を縮小。
  region 検出 (`boundary_regions`) は維持。
- **post-pass 層に `ReadingPostPass` seam を導入** (`scoring/postpass.rs`): `analyze()`
  に直書きしていた連濁 / 腹+空く 補正を adapter (`RendakuPass` / `HaraSukuPass`) 化、
  `POST_PASSES` 配列で適用順を data 化。
- **dict reading 解決を `matcher::resolve_readings` に集約**: DictBridgeProvider の
  emit に 4 重コピペされていた 「match 探索→default、 alt filter」 を 1 箇所に。
- **TOML dir loader を `loader::for_each_toml_in_dir` に統合**: rules / dict /
  loanwords の 3 経路が重複させていた walk + schema 検証 + role 解決を共通化、
  walk の 2 重実装を 1 つに。
- 設計決定を ADR-0005 (多トークン文脈補正は post-pass 層) に記録。

文脈依存読みの post-pass 追加 (patch)。

### Fixed

- **「お腹/小腹/腹 + 空く」 が 「あく」 と誤読されるバグ** (`scoring/contextual.rs`):
  `小腹が空いた` が `こばらがあいた` (空席の意) と読まれていた。 `空く` は
  あく / すく の両義で、 matcher は前後 1 token しか見ないため 2 token 離れた
  「腹」 文脈を判定できなかった。 path 確定後の token 列を走査する post-pass
  (`apply_hara_suku_inplace`) を追加し、 空く活用の直前 1〜2 token に 「腹」 で
  終わる surface があれば す-読みに補正 (= `こばらがすいた`)。 「席が空いた」
  「手が空いた」 等の非空腹文脈は `あいた` のまま不変。

production 由来の記号読みバグ修正 + 0.1.1 以降の蓄積分 (patch)。

### Fixed

- **波ダッシュ `〜` / `~` / `～` が非数値文脈で 「から」 と誤読されるバグ**
  (`scoring/numbers.rs`): `がんばれ〜` のような口語の長音・強調用途で `〜` が
  「から」 と読み上げられていた (TTS で 「がんばれ から」 になる)。 数字隣接
  (`3〜5` = range 用途) のときのみ 「から」 を採用し、 それ以外 (kana / 漢字 /
  文末) では空 reading で surface のみ消費して読み上げないよう修正。 alpha.21 の
  空白 padding 対処 (`" から "`) が結局 「から」 を発話していた regression を是正。

### Added

- **`furigana serve` の rate limit を config 化** (`config.rs` / `serve/mod.rs`):
  これまでハードコードだった per-IP レート制限 (`per_second(1)` / `burst_size(5)`) を
  `[rate_limit]` セクションで上書き可能に。未指定時は従来どおり `1` / `5`。
  高頻度の配信コメント用途や負荷試験で緩められる ([CONFIG.md](docs/CONFIG.md) 参照)。
- **負荷試験スクリプト** (`tools/k6_loadtest.js`): k6 で tts / ruby / analyze 等の
  代表ペイロードを投げ、latency / error rate を計測する。

## [0.1.1] - 2026-06-01

production signal 由来の counter / loader バグ修正 (patch)。

### Fixed

- **末尾再帰助数詞 「目」 が発火しないバグ** (`scoring/numbers.rs`): `2個目` が
  `ニコモク`、 `5人目` が `ゴヒトメ` のように、 counter regex が base 止まりで
  末尾 「目」 が単漢字 fallback の 「モク」 に落ちていた。 `(NUM)(base)(目)?`
  構造化で算用数字を、 `(KANJI_NUM)(base)(目)` で漢数字 (`一個目` 等) を修正。
  bare 漢数字 + 助数詞 (`一日` 等) は従来通り Lindera に委譲 (chunker 互換維持)。
- **detailed entry sub-table に吸収された bare entry の silent drop** (`dict.rs`):
  `[entries."X"]` の match block 後に書かれた `"完治" = "カンチ"` 等の simple entry が
  TOML 仕様で match block table に吸収され、 `EntryDetail` deserialize 時に未知 field
  として捨てられていた (dict 側で 184 件が dead、 `完治 → カンジ` 等が露呈)。 loader が
  entry value tree を再帰走査して非 ASCII string key を top-level simple entry に
  hoist 救済 (explicit entry 優先・非破壊)。 dict 作者は entry 並び順を気にせず
  append できるようになる。

## [0.1.0] - 2026-05-12

alpha.10〜.21 の累積 work を 0.1.0 stable として cut。 主要 corpus 99.2% / extended.toml
stress 73.4% / OpenJTalk 1000 件比較 83-85% / VOICEVOX engine 比較 75-77%、 production 品質に到達。

### alpha.21 (= dict 改善 sweep + 公開 API wrapper + lib bug fix)

**dict 改善 round 31-46 (= 16 round)**:

- joyo の動詞訓読み default 偏向を 30+ 字 sweep → `[[kanji]]` block 文脈ルールへ統合
  (= 卑怯者/前+動詞/入/来/客/2人/2桁/方/日/変/速い/大剣/実写/斬り/方/日/蒸/足/煽/第/寝/間/割/内/体/台/服/側/別/化/眼/説/逆/型/確/長/起動中/笑笑/員/弱/圧/小/辛/達/数/毒/急/即/今+数字/凸/真逆/煽れ/N+中接尾辞/同字異義姓 46 件/勢/市/体接尾辞)
- jukugo workaround 22 件 → 2 件に圧縮 (= round 35 sweep、 「行っ = イッ」 アプローチで Lindera 抗えない case のみ jukugo)
- `next_eq` matcher の挙動理解 (= ひらがな連続全体と比較する仕様、 1 char match は `next_starts_any` を使う)
- 人名 jukugo 46 件追加 (= 田中/鈴木/山田/水田 ... 主要姓、 配信チャット 「○○ さん」 同字異義誤読 fix)
- regression test 242 件 / 100% pass 維持

**公開 API wrapper** (= わんコメ等 配信向け OSS 外):

- ja-furigana lib + DB dict 上書き → ja-furigana lib 一本化 (Phase B、 DB 参照削除)
- `signal_log` 機能新規: 改善 signal を専用 file (`improve_signal_NNN.jsonl`、 size 10MB rotation)
  に集計 counter 出力、 raw コメント全文は構造的に保存しない privacy-safe 設計
- `raw_context` opt-in 機能: fallback hit token の前後 window N=2 を `raw_context_NNN.jsonl`
  に保存 (= dict 改善判断材料、 全文不保存)
- 自動 dict 更新 task: 起動時 + 定期 (= `FURIGANA_DICT_UPDATE_INTERVAL_SECS`) で GitHub Release
  から最新 dict tarball 取得 + atomic swap

**lib bug fix (= alpha.21 commit)**:

- 半角 space (U+0020) 含む input で `solve_path` が空 path 返す致命 bug fix
  (= `to_hiragana / to_ruby / to_tts / analyze` 全 mode で空 output だった)、
  `AlphabetPassthroughProvider` で半角 space passthrough + `preprocess_input()` で
  半角 space → 全角 space 変換の 2 段 fix
- `tower_governor::PeerIpKeyExtractor` の ConnectInfo 抽出失敗 → 500 「Unable To Extract Key!」
  fix: `axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())`
  に変更で peer IP extension が inject (= localhost 直叩きでも rate limiter が正常動作)

### alpha.20 (= 形態素信頼 band-up、 dict 未登録の純漢字熟語救済)

- `BAND_LINDERA_COMPOUND = 150` を新設 (= `Score::lindera_compound(length)` constructor)、
  Lindera が **2 字以上 + 全 char 純 CJK 統合漢字** の surface を 1 token で返したとき band を
  50 → 150 に格上げ。 単漢字 default (band 100) 合成より形態素 1 token を優先する
- 適用範囲は厳格に絞り: 単漢字 surface (= 「私」) / 漢字+okurigana 混在 (= 「来た」) /
  々/〆/ヶ を含む surface (= 「我々」、 OdorijiProvider 担当) は band 50 維持
- band 序列: `1000 (dict) > 950 (special) > 150 (lindera_compound) > 100 (kanji default) > 50 (lindera)`、
  dict / 特殊処理 (数字 / 助数詞) には常に負ける = dict 整備が source of truth
- 効果: extended.toml stress corpus で **56.4% → 73.4% (+17 pt)**、 主要 4 corpus
  (should_read / general / sentences / touhou) は 99.2% を維持 (= 既知 2 件 fail
  は連濁 / 助数詞 related で band-up と無関係)
- alpha.18 で試した 「漢字+okurigana の band-up trick」 を撤回した経緯と整合: あれは
  送り仮名読みを context rule で書くべき問題、 今回は dict 未登録 2 字熟語で 単漢字
  default が形態素 1 token を握り潰す現象への対症 = 動機 / 適用範囲ともに別問題



### 主要 milestone (= alpha.10〜.19 累積)

- **Smart engine 投入 + Strict 削除** (alpha.10〜.15、 -3000 行): 旧 priority chain
  pipeline を撤廃、 6 provider + Viterbi DP + band lexicographic 比較 で全 reading 解決
- **dict format 完全再編成** (alpha.10〜.11): `[meta] schema_version = "2"` 必須化、
  `[entries."X".match]` (inline match) + `[[kanji]]` block (= 文脈分岐 reading) の
  declarative format に統一
- **Lindera fallback** (alpha.13): band 50 の safety net、 helper / okurigana / dict 未登録の
  surface を救う
- **dict-curated 路線統一** (alpha.19): lib 側 band hack を撤回、 動詞 / 形容詞 1 字を
  [[kanji]] block + next_starts match で declarative に書く方針 (= OSS curation loop)
- **UniDic feature flag** (alpha.17): `dict-ipadic` (default) / `dict-unidic` (cwj、
  0.2.0 intonation 用 pitch accent base data) を build-time switch
- **inspect API** (alpha.19): `extract_dict_gap_candidates` で production traffic から
  dict 未登録 surface を抽出する pure 関数群 (lib は telemetry を持たず caller に委譲)

### Breaking changes (= 0.1.0 で alpha.9 → 0.1.0 移行時)

- `Engine` enum / `JA_FURIGANA_ENGINE` env var / `Furigana::engine()` / builder `.engine()`
  全削除 (alpha.15)、 Smart engine が唯一の path
- 旧 `reading::pipeline` / `reading::context` / `chunks` / `loanwords` / `single_overrides` /
  `numbers::phrase` modules を削除
- `FuriganaBuilder::core_loanwords_dir()` / `.single_overrides_file()` 削除
- `furigana-diff-engines` bin 削除 (= Strict vs Smart 比較が無意味化)
- `rules::context` data type 削除 (= dict 側 [[kanji]] / [[entries]] match で代替)

### corpus 正解率 (alpha.20 末)

| corpus | IPADIC | UniDic |
|---|---|---|
| should_read.toml (150) | 99.3% | ~99% |
| general.toml (33) | 97.0% | 97.0% |
| touhou.toml (30) | 100% | 100% |
| sentences.toml (49) | 100% | ~96% |
| **小計 (主要 262)** | **99.2%** | ~97% |
| extended.toml (94、 stress) | **73.4%** (alpha.19 末 56.4%) | — |

参考: production reference (ryuuneko.com) は extended.toml で 75.5%、 残 2 件差
まで肉薄。 「両方不正解」 ≒ 共通 dict gap (= 慣用句 / 古文系) は OSS curation で
漸進的に詰める射程。

### API stability policy (= 0.1.0 stable で約束)

0.1.0 stable cut 後、 以下の **public API は SemVer 互換維持** (= 0.1.x patch で
breaking change なし、 0.2.0+ で additive 拡張のみ):

**主要 public 型 (= lib が return する output 型)**:
- `Furigana` / `FuriganaBuilder` — method 削除 / signature 変更なし
- `AnalyzeResult` / `AnalyzeToken` — `#[non_exhaustive]`、 field 追加可
- `Candidate` / `Score` — construction は `::new()` 経由のみ stable
- `ContextWindow` / `DictGapCandidate` — `#[non_exhaustive]` (= 後者のみ)
- `ReadingToken` — field 構成 fix
- `RomajiStyle` / `TtsOptions` — variant 追加可 (= `#[non_exhaustive]` enum 想定)

**主要 public 関数**:
- `Furigana::analyze` / `tokenize` / `to_hiragana` / `to_ruby` / `to_tts` / `to_romaji`
- `extract_dict_gap_candidates` / `surface_with_context` / `token_band`
- `tokens_to_hiragana` / `tokens_to_ruby` / `hiragana_to_romaji`

**unstable surface (= 0.1.x で変更余地あり)**:
- `pub mod` で再エクスポートしている内部 module (`scoring::engine`, `scoring::format` 等)
- dev tool bin (`furigana-corpus-check` / `furigana-analyze-one`)
- 内部 helper 関数 (= 上記の 「主要」 リストにないもの)

### benchmark (alpha.19、 Windows 11、 IPADIC default)

| input | `analyze()` raw | `to_ruby()` full pipeline |
|---|---|---|
| short (18 B) | 9.6 µs | 10.4 µs |
| short_phrase (27 B) | 9.7 µs | 9.5 µs |
| medium (110 B) | 60.4 µs | 63.0 µs |
| long (400 B) | 287 µs | ~290 µs |

= alpha.13 で測った baseline (= Strict 切替前) と同等の latency。 dict-curated
context rule 路線 (= 30+ [[kanji]] block 追加) で path scoring overhead は起こらず、
0.1.0 stable cut で latency target を維持。

## [0.1.0-alpha.19] - 2026-05-12

**band up trick 撤回、 dict-curated 路線 (= [[kanji]] block + next_starts match) に統一**。

設計判断: alpha.18 の Lindera band up は **band hack** で、 「なぜこの読みか」 が
opaque。 ja-furigana の scoring-engine.md 原則 「答えを持つ辞書」 「OSS curation
loop」 「機械学習なし」 と整合しない。 user 指摘 「送り仮名は文脈ルールで書けば
よくね」 への対応として、 declarative な dict context rule 路線に統一。

### Changed

- `LinderaFallbackProvider::candidates_at`: band up logic (= alpha.18 の
  `is_kanji_okurigana_form` で band 100 に bump) を撤回、 一律 band 50 に
  戻す。 Lindera fallback は 純粋な safety net (= 他 provider が一切覆わない
  位置のみ採用) の責務に戻る

### Notes (dict 側 commit に同期)

- 動詞 / 形容詞 13 字を `[[kanji]] block` 化 (declarative context rule):
  美 / 食 / 飲 / 走 / 高 / 楽 / 苦 / 早 / 古 / 低 / 忙 / 読 / 遅
- 各 block は `default = on-yomi` + `[[kanji.match]] next_starts = "..."` で
  okurigana 先頭文字を見て kun-yomi stem に分岐
  - 例: 美 default ビ + next_starts "し" → ウツクシ (= 美しい / 美しかった / 美しさ)
  - 例: 食 default ショク + next_starts "べ" → タベ (= 食べる / 食べた / 食べます)
- 「来」 は ka 変動詞で活用ごとに母音変化 (キ/ク/コ) で alpha.18 既追加、 alpha.19
  でも維持

### Validation

corpus (lib furigana-corpus-check):

| corpus | IPADIC | UniDic |
|---|---|---|
| should_read.toml (150) | 98.7% | 97.3% |
| general.toml (33) | 97.0% | 97.0% |
| touhou.toml (30) | 100% | 100% |
| sentences.toml (49) | 100% | 95.9% |
| 計 (262) | 98.9% | 97.3% |

= alpha.18 と同等以上の精度を、 band hack 抜きで実現。 「なぜこの読みか」 が
dict TOML を見れば 1 行で確認可能、 contributor PR で改善可能。

## [0.1.0-alpha.18] - 2026-05-12

**動詞 / 形容詞 活用形の 汎用解 (= LinderaFallbackProvider band up)**。
alpha.17 で hand-coded した 9 個の活用形 entry (= 来た / 来る / 大きい / 等) を
削除、 lib 側 1 行 logic で全動詞 / 全形容詞 活用に汎用対応。

設計判断: ja-furigana の主用途 = 漢字ルビ振り で、 個別 entry を手書きするのは
**スケーラブル ではない**:
- 全動詞 × 全活用形 (10+ forms) = 数万 entry 増殖
- 辞書メモリ / 探索効率を圧迫
- dict 編集の負担増

代わりに **LinderaFallbackProvider で 「漢字 + okurigana」 surface に band up** を
追加 (★alpha.18)。 Lindera が 1 token として返す活用形 (= 大きい / 美しい /
食べる 等) は band 100 で出て unihan per-char fallback (band 100) に length で勝つ。

### Added

- `LinderaFallbackProvider::is_kanji_okurigana_form` 判定: surface が 漢字始まり +
  ひらがな終わり ( ≥2 char) なら band 100 で emit、 それ以外は band 50 のまま
- 効果: 全動詞 / 全形容詞 / 全副詞 活用形が dict 個別 entry 不要で正しく読まれる

### Notes

- 来 のみ ka 変動詞で活用ごとに母音変化 (キ / ク / コ)、 Lindera が一貫した
  1 token を返さないため [[kanji]] block で個別対応 (dict 側 commit)
- alpha.17 で追加した 9 個の活用形 entry を削除、 dict 側 commit で同期

### Validation

corpus (lib furigana-corpus-check):

| corpus | IPADIC | UniDic |
|---|---|---|
| should_read.toml (150) | 98.7% | 97.3% |
| general.toml (33) | 97.0% | 97.0% |
| touhou.toml (30) | 100% | 100% |
| sentences.toml (49) | 100% | 95.9% |
| 計 (262) | 98.9% | 97.3% |

= alpha.17 末と同等以上の精度を、 dict -9 entries で実現。 「汎用性なさすぎ」
user 指摘への直接対応。

## [0.1.0-alpha.17] - 2026-05-11

**UniDic feature flag 追加 + 自然文 corpus 拡充**。 ja-furigana が漢字ルビ振り
用途で IPADIC / UniDic どちらでも tie で動作するか実証する experimental。

### Added

- `dict-unidic` feature flag (= `lindera-unidic` 経由で UniDic 現代書き言葉 cwj
  embed)。 default は `dict-ipadic` (= 既存挙動維持)、 caller が build-time で選択可
  ```bash
  cargo build --release --features dict-unidic --no-default-features
  ```
- `kana::normalize_long_vowel`: UniDic 発音形 (= 「ガッコー」 「オオキー」) を
  表記読み (= 「ガッコウ」 「オオキイ」) に正規化、 ア/イ/ウ/エ/オ 5 段全対応
- `furigana-corpus-check` bin: corpus regression / 辞書比較用 dev tool。
  expected match rate を集計、 IPADIC / UniDic 比較や dict 改善前後の精度比較に使う

### Changed

- `tokens_to_hiragana` / `tokens_to_ruby`: surface が **全部 kana** の場合は
  surface をそのまま使う (= 「こんにちは」 + UniDic pron 「コンニチワ」 で
  「は」 → 「わ」 と変換されない、 user の表記を尊重)

### Validation 結果

experimental corpus pass (lib furigana-corpus-check):

| corpus | IPADIC | UniDic |
|---|---|---|
| should_read.toml (150) | 94.7% | 94.7% (±0) |
| general.toml (33) | 97.0% | 97.0% (±0) |
| touhou.toml (30) | 100% | 100% (±0) |
| sentences.toml (49、 新設) | 71.4% | 67.3% (-4.1pt) |
| **総計 (262)** | **91.2%** | **90.5%** (-0.7pt) |

= **漢字ルビ振り用途では IPADIC ≈ UniDic で tie**。 自然文での -0.7pt は UniDic
の 発音形 と corpus 期待値 (表記読み) のズレ。 0.2.0 intonation で UniDic の
pitch accent (aType) を取りに行く時に再評価予定。

### Notes

- dict-unidic は **experimental feature**、 default に切り替える計画は 0.2.0+
- corpus sentences.toml (49 cases) を新設、 dict 拡充 / engine 比較の baseline に
- 残 failure は両 engine 共通の **jukugo coverage 不足** (来た / 学校 / 駅 / 会議 /
  毎日 / 週末 / 勉強 / 綺麗 等)、 dict 拡充で解消可能

## [0.1.0-alpha.16] - 2026-05-11

**alpha.15 (Strict 削除) 後の cleanup release**。

### Removed (= dead code 削除)

- `rules::context` module + `RulesData::context` field — context match は alpha.11+ で
  dict 側 `[entries."X".match]` / `[[kanji]]` block に移行済、 lib 側は読み込んで
  いただけで未使用だった
- loader の `Some("context") =>` 分岐: `ContextData::merge` 呼び出しを削除、
  silent skip に変更 (= 古い release dict 互換のため role 認識は維持)
- bench `Strict::to_ruby` ラベル → `to_ruby_full` にリネーム (= Strict はもう無い)
- `furigana-cli/paths.rs` の `loanwords_dir` / `single_overrides_file` 未使用 method 削除

### Changed

- `Furigana::fmt` (Debug impl) の `context_rules` field → `counters` 数を出すよう変更
- `docs/PROPOSALS/scoring-engine.md`: Status を 「Implemented and shipped in
  alpha.15」 に更新、 実装 milestone 一覧を追加
- `docs/ARCHITECTURE.md`: lib 構成図 + パイプライン詳細を Smart engine 一本化後の
  状態に更新 (= 8 段階 → 6 provider + Viterbi DP)
- `dict.rs` の doc comment: 削除済 module への dead リンクを修正

### Notes

- Strict 削除 (alpha.15) で見落としていた dead code を一掃。 lib test 379 passed /
  clippy clean / build 通る。
- 0.2.0+ で再統合予定: `loanwords` (AlphabetPassthrough 統合)、 `numeric_phrases`
  (Smart provider 化)、 `single_overrides` 相当 (= 既に dict 側 `[[kanji]]` block で代替済)

## [0.1.0-alpha.15] - 2026-05-11

**Smart engine 完全移行 + Strict engine 削除** (= alpha.10〜.14 work + alpha.15
の Strict cleanup を bundled release)。 alpha.9 から飛ぶ場合は major breaking
change — 既存 `Engine` enum / env var / Strict pipeline 関連 API 全削除、
Smart engine が唯一の reading 解決経路となった。

### Breaking changes (alpha.15 で投入)

- **`Engine` enum 削除**: `scoring::candidate::Engine` (Strict / Smart variants)
  を完全削除。 caller の `.engine(Engine::Smart)` / `.engine(Engine::Strict)` /
  `Furigana::engine()` getter / `JA_FURIGANA_ENGINE` env var /
  `resolve_engine_from_env` 関数 すべて削除。 Smart engine が唯一の path。
- **Strict pipeline 関連 module 削除**:
  - `reading::pipeline` (`resolve_reading`)
  - `reading::merge`
  - `reading::context` (`apply_context_rules`) — context match は dict 側の
    `[entries."X".match]` / `[[kanji]] kanji.match` で表現する設計に移行済 (alpha.11+)
  - `chunks` module (NumberChunker) — Smart の `NumberCandidateProvider` がカバー
  - `loanwords` module — 0.2.0+ で `AlphabetPassthroughProvider` に統合予定
  - `single_overrides` module — dict 側 `core/kanji/*.toml` の `[[kanji]]` block で代替
  - `numbers::phrase` (NumericPhraseMatcher) — 既知の coverage gap、 0.2.0 で
    Smart provider 化予定
- **`FuriganaBuilder` API 簡素化**: `.engine()` / `.core_loanwords_dir()` /
  `.single_overrides_file()` 削除。 `.rules_dir()` / `.core_dict_dir()` /
  `.user_dict_dir()` / `.overrides_file()` / `.add_entry()` のみ残存。
- **`furigana-diff-engines` bin 削除**: Strict vs Smart 比較が無意味になったため。

### Migration (alpha.9 → alpha.15)

```rust
// 旧 (alpha.9 以前)
let f = Furigana::builder().engine(Engine::Smart).build()?;

// 新 (alpha.15+)
let f = Furigana::builder().build()?;
```

env var `JA_FURIGANA_ENGINE=smart` を使っていた場合は削除可能 (= 常に Smart)。
`JA_FURIGANA_ENGINE=strict` を使っていた場合は挙動が変わる (= Smart engine 強制、
corpus 正解率は Smart 95.8% > Strict 93.9% で品質は上)。

### alpha.10〜.14 累積 work (= 本 release で bundled)

主軸は **scoring engine 投入 (Smart 一本化)** + **新 dict format 受け入れ
(`[entries."X".match]` / `[[kanji]]` block)** + **特殊処理 (cross-cutting)
provider 化** + **Lindera fallback (band 50 safety net)** + **`to_*` 経路を
Smart engine に wire-up**。
★ 番号は [`docs/PROPOSALS/scoring-engine.md`](./docs/PROPOSALS/scoring-engine.md)
の確定 marker。

公開 API は backwards compat 維持 — 既存 `to_hiragana` / `to_ruby` / `to_tts` /
`to_romaji` の挙動は不変、 Smart engine は env var `JA_FURIGANA_ENGINE=smart`
または `FuriganaBuilder::engine(Engine::Smart)` で opt-in する experimental flag。
0.1.0-rc1 で Smart に default 切替予定 (期日 driven ではなく lib readiness 駆動)。

**dict format breaking change** (★A1b、 alpha.10〜): `load_rules_dir` /
`Dict::from_toml_file` / `Dict::from_toml_dir` / `Loanwords::from_toml_file` /
`Loanwords::from_toml_dir` / `SingleOverrides::from_toml_file` で `[meta]
schema_version = "2"` を **必須化**。 旧 alpha era format (= field 不在 or `"1"`)
は明確 Validation error で reject、 caller / contributor に migration を促す
(error message に `furigana-dict/tools/migrate_v2.py` の path 入り)。 dict 側の
v2 stamp は coordinated に E1 / alpha.11 dict release で実施 (lib alpha.10 公開
時点では shipped dict v2026.05.09 と非互換、 alpha.11 dict release との pair で
利用想定)。

2026-05-11 更新: **alpha.10 release 自体 skip** 方針 (= GitHub release 含めて
出さず、 4 commit は master push 済の内部 milestone label として残す)。 次の
release は alpha.11+ で alpha.10 work + alpha.11 work を一括公開。 crates.io
publish は依然として 0.1.0 stable まで休止。 dict 側 E1 (= schema_version stamp)
は coordinated に furigana-dict v2026.05.11 として release 公開済。

### Added (scoring engine: Smart engine 投入 + 新 dict format 受け入れ)

- **`crate::scoring` module 新設** — 新 Smart engine の全 component を集約:
  - `candidate.rs` — `Score` (4 軸 lexicographic 比較: band → length →
    match_hits → boundary_penalty)、 `Candidate` (input 上 byte range +
    reading + score の 1 edge)、 `CandidateProvider` trait (provider 各層が
    `candidates_at(pos)` で候補列挙)、 `Engine` enum (Strict / Smart)、 band 定数
    (`BAND_DICT_EXACT`=1000 / `BAND_SPECIAL`=950 / `BAND_PROTECTED`=2000 /
    `BAND_KANJI`=100 / `BAND_LINDERA_UNIHAN`=50)。 ★B2
  - `engine.rs` — `PathScore` (weakest_band + edge_count + total_match_hits +
    total_boundary_penalty 集約) + `solve_path` Viterbi-like DP で input 全体の
    最良 candidate path を解く。 longest match は edge_count 軸で表現 (= 純 sum
    集約だと細分割 path が勝つ問題を回避)。 ★B1
  - `boundary.rs` — `KanjiRegion` + `BoundaryAnalysis::analyze(input,
    region_has_exact_match)` で漢字連続 region を検出、 (b) 完全一致 entry あり
    = -300 / (c) 漢字 3 文字以上で完全一致なし = -600 ペナルティを割り当て。 ★B3
  - `format.rs` — 新 dict format の struct 定義: `Entry` untagged enum で簡略形
    (string) / inline (`{ reading=..., match=[...] }`) / expanded sub-table
    の 3 形式併存、 `EntryDetail` + `MatchBlock` + `MatchCondition` (literal +
    char_type のみ、 **品詞 matcher 不採用**) + `CharType` enum + `KanjiBlock`
    first-class candidate generator + `EntriesData` / `KanjiData` wrapper。
    Deserialize struct 定義のみ、 これら新 struct を実際の dict load 経路で使う
    wire-up は dict v2 stamp が出揃った後 (alpha.11+) に実施。 ★A2
  - `matcher.rs` — `MatchContext` + `matches_context()` + `classify_char()` で
    literal + char_type 評価 (Lindera 撤廃路線整合、 `prev_pos` / `next_pos` 不採用)。 ★A3
  - `bracket.rs` — `strip_intonation_markers` で reading 内 `[`、`]`、`/` を除去
    (0.2.0 intonation 機能の forward compat、 dict は 0.1.0 から bracket 書き
    OK、 lib は strip して reading のみ使用、 0.2.0 で activate 予定)。 ★D
  - `special.rs` — `ProtectTokenProvider` (URL / Email / 絵文字を band 2000
    candidate として透過、 reading = surface) + `AlphabetPassthroughProvider`
    (英字 token を case-fold + 全角→半角 normalize 後 lookup map 完全一致 →
    band 1000、 miss 時は surface passthrough を band 100 fallback で出す)。
    miss が band 100 まで下がるのは `NumberCandidateProvider` (band 950) と
    競合する `100km` のような alphanumeric mixed surface で SI reading を
    勝たせるための調整 (proposal §5.6 「band 比較対象外」 を fallback と解釈)。 ★C1 / C2
  - `numbers.rs` — `NumberCandidateProvider` で 数字 + 助数詞 / 大数スケール
    (+ 末尾漢字 unit) / SI 単位 / 和式日付 (`YYYY年MM月DD日` / `MM月DD日`) /
    和式時刻 (`H時M分S秒`) / 時刻 (`HH:MM(:SS)`) / 記号 1 文字 / 素の数字 を
    band 950 candidate として供給。 既存 `NumberChunker::split` の per-position
    再実装 (= chunker は左→右 greedy walk、 provider は各 byte 位置で全 pattern を
    並列に candidate 化、 path 選択は Smart engine DP に委ねる)。 jukugo super-set
    check は不要 (DP が band 1000 dict entry を自然に優先)、 「N日」 単独は期間
    扱い (days.toml 特殊読み bypass)、 日付 pattern 内は days.toml 特殊読み採用、
    漢数字 (一二三 等) の normalize は `DATE_NUM_PAT` 経由 (= 日付 pattern のみ
    で発火)。 既存 `chunks/regex.rs` と独立 implementation (= URL_RE / EMAIL_RE
    重複と同方針、 0.2.0+ で chunker 削除と coordinated に整理予定)。 ★C3
  - `odoriji.rs` — `OdorijiProvider` (々 1 文字位置で band 100 placeholder
    candidate、 reading = "々" plug) + `apply_rendaku_inplace` (path 確定後
    token 列を walk、 直前 token reading + `kana::voice_first_kana` で連濁適用)。
    既存 Strict engine の `reading::pipeline::expand_odoriji_inplace` と同じ
    rule。 神々 → カミ + ガミ (連濁あり) / 我々 → ワレ + ワレ (連濁対象外で
    そのまま複製) / dict 完全一致 entry (神々=カミガミ) なら band 1000 で勝つ。 ★C4
  - `analyze.rs` — `AnalyzeResult` / `Token` (★11 0.1.0 freeze 型) +
    `analyze(input, providers, boundary)` standalone function。 path 採択後の
    token 列 + 各位置の候補一覧 + boundary region を返す debug / inspection API。 ★F1

- **`Engine::Smart` experimental flag** — `FuriganaBuilder::engine(Engine::Smart)`
  で明示指定、 または env var `JA_FURIGANA_ENGINE=smart` (定数 `ENGINE_ENV_VAR`
  = `"JA_FURIGANA_ENGINE"`) で opt-in。 default は `Engine::Strict` (alpha
  期間中)、 0.1.0-rc1 で Smart に default 切替予定。 `Furigana::engine()`
  getter で確認可能。 ★B4

- **`Furigana::analyze()` 公開 API** — caller が candidate scoring engine の
  path 採択 / 候補 / boundary region を inspect する debug API。 alpha.10 段階
  の provider 集合は `ProtectTokenProvider` + `AlphabetPassthroughProvider::
  passthrough_only` + `DictBridgeProvider` (= 既存 `Dict` を `CandidateProvider`
  化する transitional bridge、 jukugo→band 1000 / unihan→band 100、 reading は
  intonation bracket strip) + `NumberCandidateProvider` (★C3、 band 950: 助数詞
  / 大数スケール / SI 単位 / 日付 / 時刻 / 記号 / 素の数字) + `OdorijiProvider`
  の 5 種、 path 確定後に `apply_rendaku_to_result` で 々 連濁適用。 loanwords
  lookup と numeric_phrases (二十歳=ハタチ 等) は alpha.10 段階未統合 (今後の
  continuous wire-up 予定)。 `engine()` setting に依らず 常に Smart engine
  結果を返す (= caller が明示的に Smart 解析を要求している前提)。 ★F1 / ★11

- **CLI `furigana lookup --mode analyze`** — `AnalyzeResult` を
  `serde_json::to_string_pretty` で stdout に出力する experimental mode。
  既存 mode (`tts` / `hiragana` / `ruby` / `kanji` / `romaji` / `romaji-kunrei`)
  には影響なし、 `--mode` whitelist にのみ追加。 ★F1 / ★12

- **HTTP `mode=analyze`** — `/furigana?mode=analyze` で `result` field に
  Smart engine 採択 path の reading 連結、 新 field `analyze` に `AnalyzeResult`
  全体 (tokens / candidates / path_indices / boundary_regions) を JSON で返す。
  既存 mode の response shape は維持、 `analyze` field は `mode=analyze` 時
  のみ含まれる (`#[serde(skip_serializing_if = "Option::is_none")]`)。 ★F1 / ★13

- **`furigana-diff-engines` 新 bin** — corpus TOML (`[[case]]` array of
  `input` / `mode` / `expected` / `note`) を入力に、 各 case を Strict / Smart
  両 engine で実行、 出力差分を表示する dev tool。 alpha.10〜0.1.0-rc1 期間中
  の dogfood / CI 監視用 (= 「Smart 投入で挙動破壊なし」 baseline 確認)、
  user 向けではなく dev binary、 crates.io には publish しない。 exit code は
  diff 検出時 / error 時 非 0。 ★F2 / ★6

- **`[meta] schema_version` validator + 各 caller wire-up** — `crate::loader::
  validate_schema_version(content, file)` + 定数 `SUPPORTED_SCHEMA_VERSIONS =
  &["2"]`。 旧 alpha era format (= field 不在 or `"1"` or `"99"` 等) を
  `FuriganaError::Validation` で明確 reject、 error message に migration script
  path (`furigana-dict/tools/migrate_v2.py`) を含める。 caller wire-up (★A1b):
  - `crate::loader::load_rules_dir` — rules dir 配下の全 file を validate
  - `Dict::from_toml_file` / `Dict::from_toml_dir`
  - `Loanwords::from_toml_file` / `Loanwords::from_toml_dir`
  - `SingleOverrides::from_toml_file`
  低 level の `from_toml_str` (= 直接 string を取る API) には validation を入れず、
  file 経由の高 level API のみで強制する設計 (= internal test 等で raw TOML を
  受ける経路を壊さない)。 malformed TOML (= 構文不正) は `validate_schema_version`
  が silent pass し、 後段 `parse_toml` が proper な `Toml` error を返す UX 設計。
  shipped dict (`furigana-dict v2026.05.09`) は v2 未 stamp なので alpha.10 lib
  + alpha.11 dict release の pair で利用、 lib alpha.10 単独 release 時点では
  shipped dict と非互換 (= breaking change、 release notes で明示)。 ★A1 / ★A1b / ★5

- **`kana::voice_first_kana` を `pub` 化** — 全角カタカナ reading の第 1 音を
  連濁化する helper (カ/サ/タ/ハ 行 → 濁音、 ナ/マ/ヤ/ラ/ワ/ア 行など連濁対象外
  で `None`)。 Strict engine の `reading::pipeline::expand_odoriji_inplace`
  と Smart engine の `scoring::odoriji::apply_rendaku_inplace` の両方が共有、
  重複実装を削除。 ★C4 関連の整理。

- **`AnalyzeResult` / `AnalyzeToken` re-export** — `lib.rs` から
  `furigana::AnalyzeResult` / `furigana::AnalyzeToken` (= `scoring::analyze::Token`
  のエイリアス) として直接 import 可能に。

- **`AnalyzeResult` / `Token` / `Candidate` / `Score` に `derive(Serialize)`** —
  CLI / HTTP の JSON 出力用、 ★11 freeze 型に additive 追加 (型構造 / field
  名は変更なし、 SemVer 互換)。

### Changed (postprocess の scoring engine 独立性を doc 明示)

- **`rules::postprocess` および `scoring::mod` の module doc に scoring
  engine 独立性を明示** — 「postprocess は scoring engine の score / candidate
  logic と独立、 path 確定後の output 形式 layer」 を両 module doc に併記。
  `Furigana::analyze` は postprocess を **適用しない** 方針も明文化 (= caller
  は raw token reading を inspect、 postprocess は `to_hiragana` / `to_ruby` /
  `to_tts` / `to_romaji` の本流 method のみで適用)。 ★C4 / scoring-engine.md §5.6 整合。

## [0.1.0-alpha.9] - 2026-05-08

alpha.8 から積み上がっていた累積変更をまとめてリリース。 主な軸は
**security 全 8 軸の補強** + **sanitize layer 新設** + **`[meta] role` 駆動 loader
の rules + dict 統一** + **rules 3 sub-dir 階層化** + **inline test の append-only
CI 強制** + **dict TOML format の DSL 化 (triple-quoted string)** + **days.toml の
`[entries]` block 化** 等。

公開 API は backwards compat を維持 — 既存 dict release tar (alpha.5+) は新 lib で
何も触らずに `furigana dict pull` で動作する。 `[meta] role` 無しの旧 file は
path-based 推定で fallback、 `DaysData` の旧 flat 形式も custom Deserialize で受け
入れる。 dict 側 PR (ja-furigana-dict#9) も新形式に migration 済 — alpha.9 lib +
新 dict release で最新形式の恩恵 (role tag 駆動 / triple-quoted DSL / 等)。

### Security (攻撃面: 辞書 / HTTP 入力)

- **`furigana dict pull` の archive 展開強化**:
  - download 合計サイズ上限 `MAX_DOWNLOAD_BYTES = 50 MB` (Content-Length と
    実 body の両方で post-check、 帯域 / disk DoS 防御)
  - 展開合計サイズ上限 `MAX_UNCOMPRESSED_TOTAL = 200 MB` (archive bomb 防御)
  - 1 entry サイズ上限 `MAX_PER_ENTRY_BYTES = 10 MB`
  - entry 数上限 `MAX_ENTRY_COUNT = 50,000` (大量小ファイル zip bomb 防御)
  - **entry type 制限**: Regular file / Directory のみ許可。 symlink /
    hardlink / char device / block device / fifo は **絶対 reject**
    (path traversal + sensitive file 露出の典型攻撃 vector を構造的に潰す)
- **`furigana serve` の HTTP body / rate limit**:
  - `tower_http::limit::RequestBodyLimitLayer` で body 上限 1 MB
    (巨大 POST による memory blow を防御)
  - `tower_governor` で rate limit 1 req/sec + burst 5 per IP
    (request flood / brute-force 攻撃の減速)
- **ReDoS audit**: lib 内 regex (loanword / 数値 / 日付 等) を audit、
  `regex` crate の linear-time 保証 (NFA-based、 catastrophic backtracking
  起きない) で OK と確認、 修正不要。
- **任意コード実行 audit** (辞書 + 入力経由):
  - lib + cli に `unsafe` block **0 件** → memory unsafety 経由 RCE 不可
  - shell exec **無し** → command injection 不可
  - DB / SQL **無し** → SQL injection 不可
  - eval / dynamic exec **無し** (rust に存在しない)
  - TOML deserialize は serde の `HashMap<String, String>` のみ → gadget 攻撃不可
  - 入力 text は data として扱われ regex pattern に流入する経路 **無し**
  - HTTP handler panic は axum default で 500 catch、 process は落ちない
  - 唯一の懸念: **regex bomb** (postprocess.toml / numeric_phrases.toml の
    pattern が巨大 regex として compile されると memory 消費)
    → `RegexBuilder::size_limit(10 MB)` で compile 拒否を追加
- 既存対策 (path traversal canonicalize、 SHA-256 sidecar 検証、 admin
  token 認証、 CORS layer、 `set_preserve_permissions(false)`、 server
  text 文字数 `MAX_TEXT_LEN`) は維持。
- **固定フォーマット以外の入力 audit + 追加対策**:
  - HTTP `mode` パラメータ: 既存の `normalize_mode` whitelist
    (`tts`/`hiragana`/`ruby`/`kanji`/`romaji`/`romaji-kunrei`) で OK
  - **HTTP auth token (X-API-Key / Bearer): timing-safe 比較に変更** —
    `subtle::ConstantTimeEq` で全 byte を見比べた結果に縮約。 単純 `==`
    だと一致 prefix 長が処理時間差に漏れて char-by-char 推測される
  - **GitHub API `tag_name` の strict format validate** — `[A-Za-z0-9.\-]`
    のみ・ 連続 `..` 禁止 ・1〜64 文字に限定。 `..` や `/` `:` 注入で
    別 release / 別 host を pull する攻撃を構造的に防御。 `dict pull`
    の URL 組立て前に `validate_tag_format` を必ず通す
  - **CLI `dict add` の制御文字 reject** — surface / reading に C0 制御文字
    (NULL や U+0001..U+0008 等、 `\t` `\n` `\r` 以外) を含む入力を reject。
    既存 `toml_escape` (`"` `\` `\n` `\r` `\t`) で TOML breaking char は
    既に escape 済み、 残る self-DoS 経路を構造的に塞ぐ
  - HTTP CORS Origin / GitHub JSON parse / 環境変数 path 等の他経路は
    現行 default で安全 (axum / serde / std::path の既存防御で吸収)
- **辞書 load 経路の sanitize layer** (任意コード埋め込み / 詐称防御):
  TOML 自体の deserialize で RCE は起きないが、 entries の **value** に紛れ
  込ませて間接的に害を及ぼす経路を構造的に塞ぐ。 新設 `crate::sanitize::
  sanitize_dict_value` で各 surface / reading を load 時 reject する:
  - **C0 制御文字 / DEL** (`\t` `\n` `\r` 以外) → log injection / display 破壊
    / 書き戻し時の TOML parse 全体破壊 (self-DoS) 防御
  - **Unicode bidi override** (U+202A..U+202E、 U+2066..U+2069) → Trojan Source
    攻撃 (PR review でコード意味と見た目が乖離する) 防御
  - **Zero-width / invisible char** (U+200B..U+200F、 U+FEFF) → homoglyph 詐称
    (一見同じ surface で別 entry を仕込む) 防御
  - **excessive length per entry** (1024 chars 上限) → 1 entry に巨大 string
    で OOM させる経路を塞ぐ
  - 適用先: `Dict::from_toml_str` (jukugo / unihan / works) +
    `Loanwords::from_toml_str` + `SingleOverrides::from_toml_str`
  - 公開 ja-furigana-dict (CJK + kana + ASCII + 通常記号のみ) は影響なし、
    既存 corpus 118/118 pass 確認

### Changed (dict loader: role 駆動 dispatch)

- **`Dict::from_toml_dir` / `Loanwords::from_toml_dir` を role 駆動に refactor**:
  従来 file 名 / dir 名 hardcoded skip (`single_overrides.toml` skip /
  `loanwords/` subdir skip) で識別していたが、 各 dict file の `[meta] role`
  tag を見て dispatch する形に変更。
  - `Dict` に load: role ∈ `{"jukugo", "unihan", "works"}` または role 不明
    (backwards compat: 古い release で `[meta]` 無い file を救う)
  - `Dict` から skip: role ∈ `{"loanwords", "single_overrides", "compat"}` /
    rules 系 role (`"counters"` / `"context"` / 等)
  - `Loanwords` に load: role = `"loanwords"` のみ
- **新規 helper `crate::loader::resolve_role`**: `[meta] role` → path 推定
  (rules / dict 両方) → None の優先順位で role を解決。 同 helper を rules /
  dict 両 loader が共有する。
- **dir 構造の自由化**: 同じ dir に jukugo file と loanwords file が混在しても
  正しく分離 load できるようになった。 `core_dict_dir(p)` と `core_loanwords_dir(p)`
  に同じ path を渡しても重複 load しない。
- 公開 API (`Dict::from_toml_dir` / `Loanwords::from_toml_dir`) のシグネチャ
  変更なし、 既存の dict release tar (alpha.5+) は path-based fallback で
  そのまま動作する。
- 関連 test 5 件追加 (dict 3 / loanwords 2): role tag 駆動 + path-based
  back-compat の両経路を validate。

### Changed (context rule: triple-quoted string で string list を受ける)

- **`prev_ends` / `next_starts_any` / `next2_starts` field に triple-quoted
  string 形式を追加**: 従来の TOML array (`["a", "b", "c"]`) に加えて、
  triple-quoted string (`"""\na\nb\nc\n"""`) でも書けるようになる。 後者は
  newline split + trim + 空行 filter で `Vec<String>` に変換される。
- **目的**: contributor が array で各行末に `,` を付ける friction を削減。
  特に多行 array (10+ entry) で merge conflict 耐性も向上 (1 行 1 entry)。
- 旧形式 (TOML array) は引き続きサポート (`#[serde(untagged)]` enum で両受け)、
  既存 dict release tar との backwards compat は維持。
- 関連 test 4 件追加: triple-quoted / blank line filter / array back-compat
  / empty string の各経路を validate。

### Changed (days.toml 構造: `[entries]` block 推奨、 旧形式互換維持)

- **`DaysData` を `[entries]` block 形式に migration、 旧 flat 形式も引き続き
  サポート**: 従来 transparent HashMap (top-level に `"1" = "ツイタチ"` 直書き)
  だったため `[meta] role` block を併置できず、 role 駆動 loader の対象外
  だった。 alpha.9 から `[entries]` table 内に entries を移し、
  `[meta] role = "days"` + `description` を併置可能に。 これで days.toml も
  他 rule file と同じく role tag 駆動で識別できる。
- 推奨形式 (alpha.9+):
  ```toml
  [meta]
  role = "days"
  description = "1〜31 日の特殊読み (1→ツイタチ 等)"

  [entries]
  "1" = "ツイタチ"
  "2" = "フツカ"
  ```
- 旧形式 (flat top-level、 alpha.5 〜 alpha.8 互換) も引き続き受け入れる:
  ```toml
  "1" = "ツイタチ"
  "2" = "フツカ"
  ```
  → custom Deserialize impl で `[entries]` key を見つければ Table 配下、
  無ければ top-level table 直下を採用する。 既存 dict release tar
  (alpha.5+) で `furigana dict pull` した user は何もせずに alpha.9 に
  upgrade できる。
- `DaysData` struct: `pub struct DaysData(pub HashMap<String,String>)` →
  `pub struct DaysData { pub entries: HashMap<String,String> }`。 `get` /
  `len` / `is_empty` の API は据え置き、 内部実装のみ `self.0` →
  `self.entries`。 derive(Deserialize) → 手書き impl に変更 (両形式 dispatch
  のため)。

## [0.1.0-alpha.8] - 2026-05-07

alpha.7 のリリースをやり直したもの。 binary 内容と機能は alpha.7 と実質同じ
(loanwords / 出力ルール / lookup priority / 踊り字 / SingleOverrides)。
alpha.7 を捨てた理由 2 つ:

1. **Immutable Releases policy** が repo で有効化されたタイミングで `gh release
   create` が即 immutable lock をかけ、 binary upload step が全 platform で
   HTTP 422 を返した。 release.yml を draft → finalize 構造に直したものの、
   alpha.7 tag は GitHub 内部 ledger に「使用済み」 として永久登録され、 同名
   tag の再 create が `Cannot create ref due to creations being restricted`
   で reject された (immutable releases を OFF にしても解除されない)。
2. **個人 email** が SECURITY.md / Cargo.toml workspace authors に直書き
   され alpha.6 と alpha.7 の crates.io author metadata に焼き付いた。 git
   history は filter-repo で全 commit から除去 + force push で消したが、
   crates.io publish 済み metadata は変えられないため、 alpha.6 / alpha.7
   は yank で対処。 alpha.8 から `mail@ryuuneko.com` author で再 publish。

機能差分は **無し** (alpha.7 と同じ)。 ci(release.yml) の draft + finalize 構造
だけが追加で含まれる。 詳細な機能変更点は下記 alpha.7 セクションを参照。

### Changed (release workflow)

- `.github/workflows/release.yml` を Immutable Releases policy 互換に修正:
  - `create-release` で `gh release create --draft` (publish 直後の immutable
    lock を回避)
  - 新 `finalize-release` job で binary 全 platform upload 完了後に
    `gh release edit --draft=false --latest` で publish 化

### Yanked (crates.io)

- `ja-furigana@0.1.0-alpha.6`, `ja-furigana@0.1.0-alpha.7`,
  `ja-furigana-cli@0.1.0-alpha.6`, `ja-furigana-cli@0.1.0-alpha.7` を
  cargo yank 済み。 alpha.5 以前は author 欄に問題があるため新規利用は
  alpha.8+ を推奨 (alpha.5 以前の yank はしない、 古い author 残置)。

## [0.1.0-alpha.7] - 2026-05-07 (yanked, see alpha.8)

下記内容は alpha.8 にも全て含まれる (alpha.7 と alpha.8 は機能同一)。 alpha.7
tag / GitHub Release は immutable ledger 残置 (再 create 不可)、 crates.io
は yank 済み。

外来語 (loanwords) サポート + 出力ルール仕様変更 + lookup priority 強化 +
踊り字「々」 自動展開 + 単漢字 default override + 検証ループで発見した動詞活用
系 bug の dict 側修正 を取り込んだ大型リリース。 alpha.6 で欠けていた Docker
image build もこの release で復旧する (MSRV 1.89 bump 込み)。 まだ alpha 中なので
公開 API は破壊的変更ありえる点に注意。

### Changed (MSRV)

- **MSRV を 1.88 → 1.89 に bump**: rustyline 18 (alpha.4 で取り込み) が
  std::fs::File::lock に依存するようになり、これが 1.89 で安定化した機能のため。
  alpha.6 release の Docker build (rust:1.88-slim ベース) が `file_lock` 不安定
  エラーで失敗していた問題への対応。
  - `Cargo.toml` workspace `rust-version`: 1.88 → 1.89
  - `Dockerfile` builder image: `rust:1.88-slim` → `rust:1.89-slim`
  - `README.md` MSRV badge: 1.88+ → 1.89+
- alpha.6 GitHub release は binary upload (5 platform) は完了済、Docker image のみ
  欠けた状態で残置。Docker image は次の release で復旧予定。

### Added (loanwords / IT 用語の英単語対応)

- **`Loanwords` data type** (`crates/furigana/src/loanwords.rs`):
  - `[entries]` 形式の TOML を recursive load (`core/loanwords/**/*.toml`)
  - **case-fold + 全角→半角 正規化** + **完全一致 lookup** (substring 切断ゼロ)
  - 「Kubernetes」「kubernetes」「Ｋｕｂｅｒｎｅｔｅｓ」 すべて同じ entry に hit
- **`chunks/split()` 階層 4.7** (jukugo prefix-match の後、 scale より前):
  - regex `[A-Za-zＡ-Ｚａ-ｚ][A-Za-z0-9...+#._\-]*` で英単語 chunk を **1 unit
    として丸ごと切り出し** (Lindera/IPADIC が token 単位でぶった切るのを防ぐ)
  - chunk 全体に対して loanwords lookup
    - hit → reading 確定 chunk
    - miss → ASCII surface のまま読みなしで残す (Lindera 経路に渡らないので
      IPADIC 推測誤読も発生しない)
- **`Furigana::builder().core_loanwords_dir(p)`** API 追加
- **`<data_dir>/data/loanwords/`** を CLI auto-load (`furigana lookup` /
  `furigana serve` 等で透過的に使える)
- **`Dict::from_toml_dir` の再帰 walk から `loanwords/` を除外**:
  - これは ASCII surface 専用で `Loanwords` 側で別管理されるため、 jukugo / unihan に
    混入させると jukugo prefix-match で「TypeScript」 等が誤って hit する問題があった
- 関連 GitHub issue: [#19 (closed)](https://github.com/RyuuNeko1107/ja-furigana/issues/19)

### Changed (出力ルール仕様変更: surface 文字種で reading 表記を切替)

`reading::output::tokens_to_hiragana` の出力ルールを surface 文字種で分岐:

- **漢字を含む surface** → reading をひらがな化 (既存挙動)
  - 「灰桜」 + ハイザクラ → 「はいざくら」
- **漢字を含まない surface** (ASCII / 全角英字 / カタカナ / ひらがな / 数字 / 記号) →
  reading を **カタカナに統一** (`hira_to_kata` 適用)
  - 「Kubernetes」 + クバネティス → 「クバネティス」 (ASCII カタカナ維持)
  - 「3」 + サン → 「サン」 (数字 chunk もカタカナ)
  - 「〜」 + から → 「カラ」 (symbols.toml の ひらがな登録もカタカナに揃える)
  - 「3本」 (漢字「本」 含む) → 「さんぼん」 (既存通りひらがな化)

これにより 「Anthropic の Claude を使う」 → 「アンソロピックのクロードをつかう」 の
ような自然な日本語混在表記が出るようになった。 ja-furigana-dict 側 corpus でも
ASCII / 数字 / 記号 を含む 4 件の expected を追従更新。

### Added (本体側 issue 起票 — 検証ループ R12-R17 で副産物として発見)

- [#13](https://github.com/RyuuNeko1107/ja-furigana/issues/13) bug: 「淹れる」 → 「いれるれる」 (送り仮名二重出力)
- [#14](https://github.com/RyuuNeko1107/ja-furigana/issues/14) bug: 「点ける」 → 「てんける」 (単漢字 unihan default が動詞活用を上書き)
- [#15](https://github.com/RyuuNeko1107/ja-furigana/issues/15) bug: unihan default が Lindera reading に override される (鋸 / 土 等)
- [#16](https://github.com/RyuuNeko1107/ja-furigana/issues/16) feat: 踊り字 「々」 の自動展開 (神々 → かみがみ)
- [#17](https://github.com/RyuuNeko1107/ja-furigana/issues/17) bug: 動詞 default reading 選択ズレ (摘む → つまむ)
- [#18](https://github.com/RyuuNeko1107/ja-furigana/issues/18) (closed) perf/lookup priority: 助数詞 / numeric_phrases が jukugo 最大マッチングを阻害 — 修正済み

### Changed (lookup priority — issue #18 解決)

- **`NumericPhraseMatcher` と `NumberChunker` に jukugo Aho-Corasick automaton を Arc 共有**:
  - phrase / counter / scale が jukugo entry の真部分集合を切り出してしまう問題を解決
  - 例: 「千本桜」 で「千本」 を numeric_phrases (千本=センボン) が先取りしていた
    → jukugo「千本桜」 を super-set check で優先採用 → 「センボンザクラ」 (連濁ザ) で出力
  - 副作用ゼロを担保:
    - **homonyms (`rules/context/*.toml` の `[[rule]] surface` 51 件) を AC patterns
      から除外** → reading pipeline の context rule (例: 「翡翠+が+水辺」 → カワセミ)
      は無傷
    - **≥3 字 jukugo のみ AC に登録** → IPADIC が一語で返す長い複合語
      (「烏賊墨」 → イカスミ、 「金平糖」 → コンペイトウ) を 2 字 jukugo
      (烏賊 / 金平) で先取り regression が出ない
- aho-corasick 1.x を依存に追加 (workspace 共有)

### Added (踊り字「々」 の自動展開 — closes #16)

- `reading::expand_odoriji_inplace` を tokenize_text の最終段に挿入。
  Lindera が 「神々」 を 神 + 々 にぶった切るのに対し、 後段で 々 token の
  reading を直前 token の reading で置き換える。
- **簡易連濁判定 `voice_first_kana`**: 直前 reading の先頭が `カ/サ/タ/ハ` 行
  なら `ガ/ザ/ダ/バ` に濁音化 (神々 → カミガミ、 国々 → クニグニ、 木々 → キギ
  等)。 `ナ/マ/ヤ/ラ/ワ/ア` 行は濁らないルール。
- 出力例:
  - 「神々」 → かみがみ
  - 「人々」 → ひとびと (ヒト + ビト)
  - 「日々」 → ひび
- 関連: [#16 (closed)](https://github.com/RyuuNeko1107/ja-furigana/issues/16)

### Added (単漢字 default override — issue #15 の限定解)

- **`SingleOverrides` data type** (`crates/furigana/src/single_overrides.rs`):
  - `[entries]` 形式 1 ファイル (`core/single_overrides.toml`) で 1 字 surface
    に対する明示的 default 上書きを管理
  - `lookup()` は内部で「surface が 1 字」 制約を課し、 ≥2 字 surface には影響
    しない (jukugo 分担を侵食しない)
- **`resolve_reading` 6 段階優先順位** に Step 4 として割り込み:
  1. 漢字なし → None
  2. context rule
  3. 熟語辞書
  4. **SingleOverrides** ← NEW
  5. Lindera reading
  6. unihan
- 「全 unihan を Lindera より先にすると副作用大」 (R20 の 6 件 corpus regression)
  が分かったので、 priority 全体を倒すのではなく **明示的に override したい
  単漢字だけ** を別 data file で管理する設計に着地。
- seed: `"土" = "ツチ"` 1 件 (ja-furigana-dict 側 `core/single_overrides.toml`)。
- 関連: [#15 (open、 限定解)](https://github.com/RyuuNeko1107/ja-furigana/issues/15)

### Security (CodeQL 起票)

- **GitHub Actions workflow に `permissions: contents: read` を明示**
  ([PR #20](https://github.com/RyuuNeko1107/ja-furigana/pull/20)、 Copilot
  Autofix 経由) — CodeQL alert "Workflow does not contain permissions" の修正。
  default token permission を最小化することで supply chain リスク低減。
- `SECURITY.md` 追加 (脆弱性報告手順、 サポートバージョンポリシー)。

### Chore

- `cargo fmt --all` + `cargo clippy --workspace --all-targets -- -D warnings`
  を pass する状態に整流 (機能変更なし、 doc_overindented_list_items /
  doc_lazy_continuation 等の lint fix)。

## [0.1.0-alpha.6] - 2026-05-07

辞書ディレクトリの再帰スキャンを **無制限階層** に拡張。これにより
ja-furigana-dict 側で `core/works/game/touhou.toml`、`core/works/anime/<title>.toml`
のような作品単位 1 ファイルの細分化構造が利用可能になる。

### Changed (`Dict::from_toml_dir`)

- 旧: 直下 + サブディレクトリ 1 階層のみスキャン (`core/jukugo/general.toml` は OK、
  `core/jukugo/works/X.toml` は読まれなかった)
- 新: `collect_toml_files_recursive` で **任意の深さ** を再帰、絶対パス順に sort、
  後勝ちで merge。配布 tar.gz の展開結果を想定するため symlink ループ対策は持たない
  (静的データ + 配布側で混入し得ない前提)

### Added (test)

- `from_toml_dir_recurses_arbitrary_depth`: `works/game/series/touhou.toml` および
  `works/anime/placeholder.toml` の 3 階層構造でロード成功と lookup ヒットを確認

### Verification

171 lib unit test + 4 doctest + 1 integration (`load_real_data`) + 2 CLI unit
全 pass、clippy clean、fmt clean。ja-furigana-dict 側 v0.1.2 (24 ファイル /
`core/jukugo/*.toml` 1 階層構造) は新 loader でも完全互換 (旧 1 階層構造は新 loader の subset)。

## 0.1.0-alpha.1 〜 0.1.0-alpha.5 - 2026-05-05〜2026-05-06 (要約)

初回 crates.io publish (alpha.1) から、 本番互換の読み解決優先順位整備 (alpha.3) /
依存 major bump 一気取り込み (alpha.4) / 辞書自動更新 admin_tokens 不要化 (alpha.5)
までを集約。 各 alpha tag の verbose な entry は
[GitHub Releases](https://github.com/RyuuNeko1107/ja-furigana/releases) を参照。

主な達成点:

- **Phase 2 機能完成** (alpha.1): `furigana repl` (対話モード) / `furigana dict pull`
  (GitHub Releases + SHA-256 検証 + tarball 展開) / ホットリロード
  (`POST /admin/reload` + Unix `SIGHUP`) / portable 配置 (`<exe>/data/` 1 階層集約) /
  SI 単位 case-insensitive lookup / `cargo about` ベースの NOTICE.md 自動生成 /
  GitHub Releases 経由の 5 platform binary + Docker image 配布
- **crate 名統一** (alpha.2): `ja-furigana` (lib) / `ja-furigana-cli` (bin) に rename、
  GitHub repo も `RyuuNeko1107/ja-furigana` / `ja-furigana-dict` に統一
- **本番互換 5 段階優先順位** (alpha.3): `resolve_reading` を
  `context rule → jukugo → Lindera → unihan` で再構成、 `Dict` を jukugo (≥2 字) /
  unihan (1 字) に内部分離。 `postprocess.toml` (Step 7 mode 別 regex 置換) 新設。
  `NumberChunker` に漢数字日付 + scale+unit 連結 + counter context (期間 vs 日付)
  対応。 検証ループ 75/75 (100%)。 CI に `audit` (cargo-audit) + `corpus` (回帰検証) job 追加
- **依存 major bump 取り込み** (alpha.4): `toml` 0.8 → 1.x / `directories` 5 → 6 /
  `criterion` 0.5 → 0.8 / `sha2` 0.10 → 0.11
- **辞書自動更新 admin_tokens 不要** (alpha.5): `furigana serve --auto-pull` フラグ +
  `[auto_update]` config (background polling、 `enabled` / `interval` / `pin`) 新設。
  `/admin/reload` HTTP は外部から同期 reload を打ちたい運用者向けに残置

その他:

- alpha.4 で **`Furigana` の Lindera 初期化を lazy 化** (`Furigana::minimal()`
  単体で 5.97 ms → 27.3 µs)、 `Furigana::merge_dict_toml` / `Furigana::preload` /
  ローマ字出力モード (ヘボン式 / 訓令式) 追加
- `crates/furigana-wasm/` (WebAssembly bindings) は alpha.4 で削除
  (`.wasm` が Lindera + IPADIC 込みで 57 MB と重く、 Web からは `furigana serve` で
  十分という判断)
- alpha.3 で `cargo test --release` harness の **51 GB alloc 暴走** を修正
  (`chunks::regex::build_alt_regex` 空 list 時の never-match pattern が release DFA
  を暴発させていた、 Lindera は無罪)

## Pre-history (Phase 1) - ~2026-05-04

- workspace 構成 (`furigana` lib + `furigana-cli` bin) と Lindera + IPADIC ベースの
  形態素解析パイプライン
- `Furigana` / `FuriganaBuilder` 公開 API、 `tokens_to_ruby` / `tokens_to_hiragana`、
  TTS 整形 (`TtsOptions` + `normalize_for_tts`)
- `furigana lookup` / `furigana serve` (Axum HTTP、 本番 API 互換) /
  `furigana dict {add,list,remove,import}` サブコマンド
- 数値テキスト全体オーケストレーション (`NumberChunker` で時刻・日付・URL・スケール・
  助数詞・SI 単位を 1 パイプラインで処理)
- データ駆動ルール: 全ルールを `ja-furigana-dict` 側 TOML で外部化
- 本番 ryuuneko.com から seed 投入 (unihan 43,749 / jukugo 605 / compat 436)

<!-- ───────────────────────────────────────────────────────────────────
     リンク参照 (Markdown reference-style links、 GitHub では invisible)
     上記 body の [Unreleased] / [0.1.0-alpha.X] の bracket 表記をクリック可能に
     する metadata。 alpha.7 は immutable releases lock + author email 焼き付き
     で yank、 alpha.1 / alpha.2 は yank 済み (rename 前 crate name)。
     ─────────────────────────────────────────────────────────────────── -->

[Unreleased]: https://github.com/RyuuNeko1107/ja-furigana/compare/v0.1.0-alpha.9...HEAD
[0.1.0-alpha.9]: https://github.com/RyuuNeko1107/ja-furigana/releases/tag/v0.1.0-alpha.9
[0.1.0-alpha.8]: https://github.com/RyuuNeko1107/ja-furigana/releases/tag/v0.1.0-alpha.8
[0.1.0-alpha.6]: https://github.com/RyuuNeko1107/ja-furigana/releases/tag/v0.1.0-alpha.6
