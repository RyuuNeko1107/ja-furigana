# Proposal: Scoring Engine v1.5 — Multi-Signal Accumulation in Same Band

**Status**: **Superseded in part by [ADR-0004](../../../docs/adr/0004-ambiguous-reading-candidates.md)** (元: Proposed 2026-05-21)

> ⚠️ **この doc は歴史的記録です。本文どおりに実装しないこと。**
> 提案後に ADR-0004 が `[[alt]]` 機構として **Phase 2 / Phase 3 を別設計で出荷済**。
> 本文の syntax / 型は実装と乖離しているため、新規実装の参照元にしてはならない。
>
> | 提案の部分 | 現状 | 正となる参照先 |
> |---|---|---|
> | **§4 Phase 2** `[[candidates]]` / `signal_score` | ❌ 不採用・出荷済の `[[alt]]` + `weight` で代替 | `scoring/format.rs` の `Alternative{reading,sense,weight}` / ADR-0004 |
> | **§5 Phase 3** `Token.alternatives` / `AltSource` enum | ❌ 出荷済 (型違い)。`AltSource` は Lindera 名公開のため不採用 | `scoring/analyze.rs` の `Token.ambiguous` / `alternatives: Vec<AlternativeReading>` |
> | **§4.7 / §10.4** 既存 `[[match]]` 全 block 再評価・順序非依存化 | ❌ 却下 (first-hit-wins を破壊、互換主張と自己矛盾) | `[[match]]` は v1 first-hit のまま温存、曖昧候補は `[[alt]]` に分離 |
> | **§6.2** lib 自動 signal (Lindera 一致 / 人名 suffix 等) | ❌ lib には入れない | 同形異音語の「選択」は `ml/` classifier が owner (`ml/CLAUDE.md`) |
> | **§3 Phase 1** match_hits を condition 数累積 | △ **唯一の未実装かつ固有価値**。別 issue に分離 | `.scratch/match-hits-condition-weighting/` |
>
> **§3.1 の事実誤認訂正**: 「現状 match block hit は `match_hits = 1`」は誤り。
> 実コードは `api.rs` で `Score::new(band, length, 0, 0)` と **match_hits を 0 でハードコード** emit している。
> よって Phase 1 は `0 → N` への変更であり、§3.5 / §8 の「100% 互換・相対順序維持」の無条件主張は成立しない
> (= `should_read.toml` corpus の before/after diff 実測を着手 gate とすること)。
>
> 降格根拠: 2026-06-03 の敵対的レビュー (5 lens + synthesis、 decision = split-minimal)。

> 関連: [scoring-engine.md](./scoring-engine.md) (= v1 archive) / [intonation.md](./intonation.md) (= 0.2.0 独立 phase) / [../ROADMAP.md](../ROADMAP.md) / [ADR-0004](../../../docs/adr/0004-ambiguous-reading-candidates.md)

## 0. 動機

v1 scoring engine (alpha.10〜.15 投入、 0.1.0 stable LIVE) は band lexicographic + inline match block で **「文脈で確定できる読み」 は強い**。 dict 改善 rolling 9 round + VV 比較 86.9% (2026-05-21) で実用域に到達。

一方、 運用で観察された **2 つの根本的弱点**:

### 0.1 候補表面化の盲点

「曖昧な読み」 (= 文脈で確定できないが両候補あり得るケース) は、 dict 側で文脈ルールが明示されていない限り **候補 list に出てこない**:

- 単漢字 default の隠れた別読み (例: 「私」 = ワタシ / ワタクシ、 「家」 = イエ / ヤ)
- jukugo 複数 reading (例: 上手 = ジョウズ / ウワテ / カミテ)
- 動詞同 surface 異読み (例: 行く = イク / オコナウ、 開く = ヒラク / アク)
- 同形異音語 (例: 明日 = アシタ / アス / ミョウニチ、 一日 = イチニチ / ツイタチ)

現状 `AnalyzeResult.candidates[i]` は **各位置で全 provider が返した candidate 一覧** を露出する (analyze.rs:69)、 これは debug 用には十分。 だが dict が `default + match block` 構造の場合、 match block が miss した surface の **代替 reading は dict から候補化されない**。 「declarative に複数 reading を宣言する手段」 が format 上存在しない。

### 0.2 match block の signal 粗さ

`MatchBlock.matches_context()` (matcher.rs:84-216) は AND 結合の **boolean 1/0**、 path 上の `match_hits` は block hit 数の単純カウント (= 0/1 binary、 candidate.rs:70-71)。 同 block 内に複数 condition を書いても **1 hit 扱い**、 condition 数 / 強度を score に反映しない:

```toml
# 強 signal (= 2 condition AND)
[[entries."辛い".match]]
prev_char_type = "漢字"
next_eq = "い"
reading = "ツライ"

# 弱 signal (= 1 condition)
[[entries."辛い".match]]
next_eq = "い"
reading = "ツライ"
```

現状この 2 block は **同じ 1 hit 扱い**、 condition 数の差が PathScore に乗らない。 contributor が 「より厳密な条件で書く」 努力が score に反映されない。

### 0.3 v1.5 の解決方向

v1 を **置き換えず additive 拡張** で 3 軸:

1. **Phase 1**: match block 内 condition を独立 signal 化 (= condition 数累積で match_hits 上昇、 内部 logic のみ、 dict 不変)
2. **Phase 2**: dict format に candidate enumeration syntax 追加 (= 複数 reading 宣言可能、 schema v2 additive)
3. **Phase 3**: `AnalyzeResult.Token` に top-K alternative reading 露出 (= 既存 `candidates` field の補強)

band lex (= calibration 沼回避) と既存 to_* API 互換は完全維持。

## 1. 設計指針

1. **既存 dict (= candidates 未宣言) は 100% 互換** — default + match block 挙動不変
2. **band lex は維持** — 連続値 score 化は依然却下、 同 band 内の差別化のみ細粒度化
3. **dict declarative を主、 lib auto signal を従** — co-occurrence / Lindera 一致 等 lib 自動 signal は段階的、 まず dict 側 declarative 力強化
4. **AnalyzeResult.Token freeze 互換** — `#[non_exhaustive]` で field 追加、 既存 caller 影響なし
5. **段階 release** — Phase 1 → 2 → 3 を別 alpha で投入、 各段階で corpus regression + VV 比較

## 2. Non-Goals (= scope 外)

- **band 自体の soft 化** (= weighted sum / 連続値 score) は依然却下方針継続
- **ML / 機械学習 weight learning** は scope 外、 hand-tune のみ
- **top-K confidence の確率正規化** は scope 外 (= rank order のみ、 score numeric は debug 用)
- **intonation / accent annotation** (= 0.2.0) との合流は coordinated だが本 doc scope 外
- **lib 側 telemetry** は OSS ローカル完結方針継続、 v1.5 でも収集なし

## 3. Phase 1 — match_hits を condition 数累積に拡張

### 3.1 現状

`MatchCondition.matches_context()` (matcher.rs:84-216) は全 condition AND の boolean。 caller (= `DictBridgeProvider` 等) は hit 時に `Score::match_hits = 1`、 miss 時に block 自体不採用。

### 3.2 v1.5 logic

各 condition (= MatchCondition の各 field) を **独立 signal** とみなし、 hit した condition 数 × weight を `match_hits` に累積。 全 condition AND 評価は維持 (= 1 つでも miss なら block 全体 no match)、 hit 時に weight 累積を計算。

| condition 軸 | weight |
|---|---|
| `prev_eq` / `next_eq` (literal exact) | **2** |
| `prev_eq_any` / `next_eq_any` (literal exact list) | **2** |
| `prev_ends_any` (literal suffix list) | **1** |
| `next_starts` (literal prefix) | **2** |
| `next_starts_any` (literal prefix list) | **1** |
| `next2_starts_any` (1 飛ばし prefix list) | **1** |
| `prev_char_type` / `next_char_type` (文字種) | **1** |
| `prev_month` / `next_digit` (述語) | **1** |

**weight 数値根拠** (= initial、 corpus dogfood で empirical 調整):
- literal exact (= 完全一致) は最も強い signal、 weight 2
- literal list (= 限定列挙) も exact 相当、 weight 2 (※ list の長さで weight 減衰は scope 外)
- suffix / prefix list / char_type / predicate は曖昧度高い signal、 weight 1
- prefix single (= `next_starts`) は exact 同等扱いで weight 2

### 3.3 具体例

```toml
# match_hits = 1 + 2 = 3 (= 「漢字直後」 + 「いで始まる」 両 condition hit、 強 signal)
[[entries."辛い".match]]
prev_char_type = "漢字"
next_eq = "い"
reading = "ツライ"

# match_hits = 2 (= 「いで始まる」 のみ、 弱 signal)
[[entries."辛い".match]]
next_eq = "い"
reading = "ツライ"
```

同 band 1000 / 同 length 内で前者が後者に勝つ → contributor が **より厳密な条件で書く動機** が score に反映される。

### 3.4 型 / 公開 API への影響

- `Score::match_hits: u8` — 0-255 range は v1.5 で十分 (= 1 block 最大 condition 数 × weight 2 = 18 程度)
- `PathScore::total_match_hits: u32` — 累積 overflow なし
- public type field 不変、 logic のみ拡張 → **fully additive、 既存 caller 影響なし**

### 3.5 既存 dict への影響

- 既存 match block (= 大半が 1-2 condition) の match_hits 値は変化するが、 **相対順序は維持** (= 多 condition block ほど強くなる方向、 弱体化はしない)
- corpus regression (`should_read.toml` 598 case) で挙動変化を逐次観測、 既存 expected reading が変わる case を発見したら weight 再調整

## 4. Phase 2 — dict format に candidate enumeration 追加

### 4.1 problem statement

現状 dict (= format.rs `Entry` / `KanjiBlock`) は **1 surface = 1 default reading + N match block**。 default を選ぶか match block の reading を選ぶかは binary。 「ある surface に 3 つ等価候補があり、 文脈次第で順位が変わる」 を declarative に書けない。

contributor は「上手 → ジョウズ default」 を書いて、 ウワテ / カミテ を `[[match]]` で文脈分岐するしかない。 結果、 文脈不在の場面では default 1 つのみが path に乗り、 他候補は `AnalyzeResult.candidates` にも出ない (= 同 provider が単一 candidate しか発しないため)。

### 4.2 新 syntax

`[[entries."x".candidates]]` array of tables を **additive 追加**:

```toml
[entries."上手"]
reading = "ジョウズ"             # default = top-1 base (signal_score 0)

[[entries."上手".candidates]]
reading = "ウワテ"               # alternative reading 1

[[entries."上手".candidates.match]]
next_starts_any = ["に", "を"]   # match hit で signal_score += 1
prev_char_type = "漢字"          # 同時 hit で signal_score += 1 (= total 2)

[[entries."上手".candidates]]
reading = "カミテ"               # alternative reading 2 (= 演劇用語)

[[entries."上手".candidates.match]]
prev_eq_any = ["舞台", "下"]    # match hit で signal_score += 2 (= literal_any weight)
```

### 4.3 semantics

- `reading` (= 既存 default field) は **base candidate**、 signal_score 0、 path 不在で top-1
- 各 `[[candidates]]` block は **1 alternative reading + match block array**
- alternative の `[[candidates.match]]` の hit 評価は §3.2 と同じ logic、 hit した condition 数 × weight が **その alternative の signal_score**
- band は base / alternative 共通 (= 1000 dict_exact)、 同 band 内で `signal_score` 大が勝つ
- match block が一つも無い alternative も valid (= signal_score 0 base alternative、 default と同 priority 競合、 TOML 出現順で順位)
- **path 選択**: base / alternatives 全部を candidate として provider が発する、 Viterbi DP は同 band lex で勝者選び、 残り top-K-1 を `AnalyzeResult.Token.alternatives` に格納

### 4.4 `[[kanji]]` block への適用

同 syntax を `[[kanji]]` block にも:

```toml
[[kanji]]
char = "私"
default = "ワタシ"

[[kanji.candidates]]
reading = "ワタクシ"             # match block 不在 = 常時候補露出、 default より弱い (signal_score 0 base)

[[kanji.candidates]]
reading = "シ"

[[kanji.candidates.match]]
prev_char_type = "漢字"           # 漢字熟語の音読み context (signal_score 1)
```

### 4.5 backward compat

- 既存 dict (= `candidates` field 不在) は **100% 互換**、 既存 default + match block 挙動不変
- `schema_version = "2"` のまま (= v2 partial extension、 v3 bump せず)
- candidates 宣言は entry / kanji block ごとに **opt-in**、 同 file 内で混在可能
- 0.2.0 で schema_version "3" bump 時に candidates を first-class 化検討 (= 本 doc scope 外)

### 4.6 dict 側 validation 追加

`furigana-dict/tools/validate.py` 拡張:
- `[[candidates]]` の reading 必須 check
- `[[candidates.match]]` の reading field **禁止** (= base candidate の reading が固定、 match は条件のみ)
- candidates 配列内の重複 reading 禁止 (= 同 surface 同 reading の重複宣言は意味ない)
- candidates と既存 `[[match]]` block の併用は valid (= 既存 match の reading は path 採択 logic で別 alternative として扱う、 §4.7 参照)

### 4.7 既存 `[[match]]` block との関係

既存 `[[entries."x".match]]` block (= reading field を持つ) は **alternative 同等扱い** に semantic 拡張:

| syntax | semantic (v1) | semantic (v1.5) |
|---|---|---|
| `reading = "X"` (default) | hit miss 時の fallback | signal_score 0 base、 top-1 候補 |
| `[[match]]` reading = "Y" | hit 時に Y を採用 (TOML 順第一 hit) | Y を alternative として candidates 同等扱い、 signal_score = condition 累積 |
| `[[candidates]]` reading = "Z" | (新規) | Z を alternative、 signal_score = `[[candidates.match]]` 累積 |

つまり既存 dict の `[[match]]` block も Phase 2 後は alternative 経路で path 評価される (= 内部 logic 統一)。 contributor 視点では `[[match]]` syntax 維持、 `[[candidates]]` は **複数 alternative + 各 alternative ごとの match 集合** を書きたい時の発展 syntax。

## 5. Phase 3 — AnalyzeResult.Token.alternatives 露出

### 5.1 現状

`AnalyzeResult.candidates: Vec<Vec<Candidate>>` (analyze.rs:69) は **provider 全列挙** で、 採択 path 以外の candidate を全部含む。 debug 用には便利だが、 「同 surface の 別 reading」 にフォーカスした alternatives は埋もれる (= 異 surface 同位置の candidate も混在)。

### 5.2 v1.5 拡張

`Token` に `alternatives` field 追加:

```rust
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct Token {
    pub surface: String,
    pub reading: String,                   // top-1
    pub range: Range<usize>,
    pub alternatives: Vec<Alternative>,    // ★v1.5 追加、 top-K-1 件
}

#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct Alternative {
    pub reading: String,
    pub signal_score: u32,                 // 0 = base (default 非採用候補)
    pub source: AltSource,                 // 「どこから来た候補か」 trace
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum AltSource {
    EntryDefault,    // [entries] default reading
    EntryMatch,      // [entries] [[match]] reading
    EntryCandidate,  // [entries] [[candidates]] reading (v1.5 新規)
    KanjiDefault,    // [[kanji]] default
    KanjiMatch,      // [[kanji]] [[match]]
    KanjiCandidate,  // [[kanji]] [[candidates]] (v1.5 新規)
    LinderaUnihan,   // Lindera unihan injection (band 50)
    Special,         // 数字 / 助数詞 / 踊り字 等 (band 950)
}
```

- 採択 reading は `token.reading` (= top-1)、 残り top-K-1 を alternatives に
- **K=3 default** (= top-2 alternatives 露出)、 caller が enumerate 全部見たい場合は既存 `AnalyzeResult.candidates` を経由
- 既存 to_* API は `token.reading` のみ使用、 alternatives 影響なし
- `#[non_exhaustive]` 効果で literal struct 構築は元々禁止、 deserialize 互換維持

### 5.3 caller view

```rust
let result = furigana.analyze("今日は上手から登場");
for token in &result.tokens {
    println!("{} → {} (top-1)", token.surface, token.reading);
    for alt in &token.alternatives {
        println!("  alt: {} (signal={}, source={:?})",
                 alt.reading, alt.signal_score, alt.source);
    }
}
```

期待出力:
```
今日 → キョウ (top-1)
  alt: コンニチ (signal=0, source=EntryDefault)
上手 → カミテ (top-1)
  alt: ジョウズ (signal=0, source=EntryDefault)
  alt: ウワテ (signal=1, source=EntryCandidate)
```

### 5.4 HTTP server schema 拡張

`GET /furigana?text=&mode=analyze` の response JSON に `alternatives` field 追加 (= additive、 既存 caller は無視):

```json
{
  "tokens": [
    {
      "surface": "上手",
      "reading": "カミテ",
      "range": [9, 15],
      "alternatives": [
        {"reading": "ジョウズ", "signal_score": 0, "source": "EntryDefault"},
        {"reading": "ウワテ", "signal_score": 1, "source": "EntryCandidate"}
      ]
    }
  ]
}
```

`mode=hiragana | ruby | romaji | tts` の response は **無変化** (= top-1 のみ使用)。

## 6. signal の catalog (= 0.1.x / 0.2.0+ 段階追加)

### 6.1 dict declarative signal (Phase 1 で導入、 §3.2)

既存 (= matcher.rs):
- `prev_eq` / `next_eq` / `prev_eq_any` / `next_eq_any`
- `prev_ends_any` / `next_starts` / `next_starts_any` / `next2_starts_any`
- `prev_char_type` / `next_char_type`
- `prev_month` / `next_digit`

新規候補 (= 0.2.0+ で検討、 本 doc scope 外):
- `when_surrounded_by` (= 前後文脈 co-occurrence、 周辺 N 文字 window)
- `when_in_jukugo` (= 形態素 boundary 信頼)
- `when_followed_by_particle` (= 助詞 boundary detector との連携)

### 6.2 lib 自動 signal (= Phase 3+ で internal 加点、 dict 不要)

各 candidate 評価時に lib が自動で signal_score に上乗せ:

- **Lindera reading 一致**: 形態素 reading と candidate reading の一致で +1 (= 形態素信頼度を soft に表現)
- **okurigana 推定強度**: 動詞活用語尾の連続一致長 (= 「行く」 + 「った」 で 2 連続 hit → +2、 [[kanji]] block の `next_starts_any` 列挙負担軽減)
- **personal_name suffix priority**: ○○さん/君/氏/様 next 文脈で前 surface の人名 candidate に +2 (= 既存 personal_names.toml の hardcoded list を lib logic 化)
- **jukugo prefix length 比**: 部分一致でも段階的に加点 (= 「紅魔館」 不在 dict で 「紅魔」 entry を発見した時、 length 比で部分点)

これらは各 candidate の `signal_score` に累積、 band lex は変えない (= 同 band 内 sort key のみ強化)。

## 7. Calibration Policy

- 全 weight は initial 1 / 2 (= literal vs predicate 2 tier の粗設定)
- weight 細分化は corpus regression (`should_read.toml`) + VV 比較で empirical 調整
- ML 的 learning は scope 外、 hand-tune のみ
- weight 表は 1 箇所 (= `matcher.rs` の const) に集約、 doc から参照可能に
- 各 release で weight 変更がある場合 CHANGELOG に明記 (= contributor の予測可能性維持)

## 8. Migration / Compat

| 軸 | v1 | v1.5 |
|---|---|---|
| 既存 dict (candidates 未宣言) | 動作 | **完全互換**、 挙動不変 |
| 既存 caller (to_* API) | 動作 | **完全互換**、 token.reading は top-1 (= default 相当) |
| `Score::match_hits` 数値 | 0/1 binary | 0-N condition 数累積 (= 数値変化、 順序は維持) |
| `AnalyzeResult.Token` 構造 | 3 field | 4 field (alternatives 追加、 `#[non_exhaustive]` で互換) |
| dict schema_version | "2" | "2" (= partial extension、 v3 bump せず) |
| HTTP response JSON | 既存形 | mode=analyze で alternatives field 追加 (= additive) |

**migration script 不要** — 既存 dict / caller / consumer は何もせず動く。 新機能は opt-in。

## 9. Release timeline (= 0.1.x patch stream)

- **alpha.22**: Phase 1 投入 (= match_hits weight 累積) + corpus regression 確認 + diff_engines で挙動変化観測
- **alpha.23**: Phase 2 dict schema 拡張 (= `[[candidates]]` field の loader + parser)、 dict 側 [[kanji]] 主要候補 (= 単漢字 top-20 別読み) sweep 試行 PR、 round 10 で VV 比較
- **alpha.24**: Phase 3 `AnalyzeResult.Token.alternatives` + `Alternative` / `AltSource` 型追加、 既存 caller 互換確認、 HTTP server schema 拡張
- **alpha.25+**: dict 主要 candidate enumeration sweep (= 単漢字 top 100 + jukugo top 100)、 round 11+ で効果検証
- **0.1.x patch**: candidates 拡充は patch 漸進、 daily-release.yml 再開後継続

各 alpha は GitHub release のみ (= alpha 期間 crates.io publish 休止方針継続)、 0.1.x patch 切替後 (= 0.1.1+) crates.io 同期 publish 検討。

## 10. Open Questions

### 10.1 top-K の K default

- **3** (= top-2 alternatives) を default 推奨、 「主要 alternative + 滅多に出ない 1 候補」 程度
- 5 / 全列挙が要る caller は既存 `AnalyzeResult.candidates` で全 provider candidate にアクセス可
- caller-side limit (= `analyze_with_top_k(k: usize)` API) は 0.2.0+ で検討

### 10.2 weight tier の粒度

initial:
- literal exact / list = 2
- suffix / prefix list / char_type / predicate = 1

4 tier (= literal_exact 4 / list 3 / suffix 2 / predicate 1) は条件強度の差を細かく表現できるが、 contributor の予測可能性が下がる。 corpus regression の diff を見て判断。

### 10.3 base alternative の露出

match block 不在の alternative (= `[[candidates]]` の reading 単独宣言) を 「常時候補」 として alternatives に露出するか:

- **Yes**: 「単漢字 default + 別読み」 を declarative に書けば常に候補化、 disambig 用途で実用的
- **No**: signal_score > 0 のみ露出、 base alternative は dict explicit 宣言の noise を防ぐ
- **default = Yes** 推奨 (= 露出する)、 caller が signal_score でフィルタ可

### 10.4 既存 `[[match]]` block と `[[candidates]]` の関係

§4.7 の semantic 拡張 (= 既存 `[[match]]` を alternative 同等扱い) は dict 既存挙動と微妙に違う:

- v1: `[[match]]` 第一 hit で reading 確定、 他 block 評価 skip
- v1.5: 全 `[[match]]` block 評価、 signal_score 高い alternative が勝つ (= 順序非依存)

これは **TOML 出現順 dependence の喪失** を意味する。 既存 dict の挙動が変わる case を corpus regression で発見したら、 weight 調整 or `[[match]]` の logic を v1 のまま残し `[[candidates]]` のみ新 logic、 の hybrid も検討。

### 10.5 lib 自動 signal の dict declarative 化

§6.2 の lib 自動 signal (Lindera 一致 / okurigana 推定 / personal_name suffix) を:
- **lib hardcoded** (= 全 entry に自動適用、 dict は知らない)
- **dict opt-in** (= entry に `auto_signal = ["lindera_match", "name_suffix"]` 等のフラグ)

initial は **lib hardcoded** (= 全 entry に自動)、 opt-out flag は 0.2.0+ で検討。

### 10.6 alternatives serialization stability

`AnalyzeResult.Token.alternatives` の順序は **signal_score 降順 → AltSource 安定順 → TOML 出現順** の tie-break:
- signal_score 降順は spec 保証
- AltSource の順序は doc 明示が必要 (= 「EntryCandidate > EntryMatch > LinderaUnihan」 等)
- TOML 出現順は dict file の物理的順序依存 → caller の test で fragile になる可能性、 doc で「best effort、 安定保証なし」 明示

## 11. References

- [scoring-engine.md](./scoring-engine.md) — v1 architecture (= shipped、 archive)
- [intonation.md](./intonation.md) — 0.2.0 stable target (= 独立 phase、 v1.5 と coordinated だが本 doc scope 外)
- [../ROADMAP.md](../ROADMAP.md) — Track A-5 として位置づけ
- 関連 memory: `feedback_no_niche_corpus_match` (= corpus 1 件 niche match 禁止) — signal 化方向と整合、 generic な multi-signal で曖昧読みを救う

---

## 次のアクション

1. 本 proposal レビュー + 確定 (= weight tier / K default / `[[match]]` semantic 拡張 の最終決定)
2. ROADMAP.md update — Track A に A-5 として追加 (= 別 session で実施)
3. Phase 1 prototype 着手 (= matcher.rs の `matches_context` 拡張 + Score::match_hits 累積、 既存 test 全 pass 確認)
4. corpus regression baseline 記録 (= alpha.21 → Phase 1 後の diff 観測 base)
5. dict 側 maintainer (= self) との coordinate (= Phase 2 schema 拡張時の dict file 改修ガイド)
