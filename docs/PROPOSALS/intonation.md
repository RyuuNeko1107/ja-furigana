# Proposal: アクセント (intonation) 機能

> **⚠️ 2026-07-04 注記: 本 doc は設計当時の proposal であり、 確定仕様は ADR が優先する。**
> 主な差分:
> - `/` phrase 区切りは **廃止** (ADR-0003: `[` `]` の 2 記号のみ、 連続 `[` が phrase 境界。 `/` は strip 互換のみ)
> - `rules/accent/` 階層・fractions rule は **0.2.0 に入れない** (ADR-0002 consequences)
> - `--mode=voicevox-aques` は lib 内 mode ではなく **別 crate `ja-furigana-voicevox`** が提供 (ADR-0001、 実装済)
> - **2026-08-11 追記**: engine adapter は 2 本立てになった —
>   VOICEVOX kana 記法 (`ja-furigana-voicevox`) と **本家 AquesTalk 音声記号列**
>   (`ja-furigana-aquestalk`、 `--mode=aquestalk`)。 両者で重複していた変換コア
>   (モーラ分割 / accent 句構築 / 助詞連結 / 記号の扱い) は lib の
>   `furigana::accent_symbols` へ集約し、 adapter には **記号の綴り方だけ** を残す
>   (ADR-0009)。 本 doc の §7.2 は voicevox 版の記法を指す
> - accent 推定は **opt-in で導入済** (ADR-0007: `estimate_accent(true)`、 外来語 -3 rule / 人名 rule、
>   `estimated: true` marker)。 「推定しない」 は default 挙動の話に変わった
> - dict bracket は **UniDic aType からの offline 生成 tool** (`furigana-dict/tools/gen_accent_brackets.py`)
>   で機械蓄積する (手書き PR 前提は撤回、 2026-07-04 に第 1 弾 3,122 entry 適用済)

**Status**: **Shipped in 0.2.0** (2026-07-06 release。 経緯: Postponed → Planned for 0.2.0 [2026-05-10] → 出荷。 確定仕様との差分は冒頭注記の ADR 参照)
**Target**: **0.2.0 stable** (0.1.0 stable cut 後の次 minor stable)
**Scope**: 「読み」 だけでなく 「東京式アクセント核位置」 も扱える ja-furigana lib への拡張、 辞書側韻律対応

> 関連: [ROADMAP.md](../ROADMAP.md) / [ARCHITECTURE.md](../ARCHITECTURE.md) / [scoring-engine.md](./scoring-engine.md)
> 関連 (dict 側): `furigana-dict/docs/SCHEMA.md` (notation 仕様)

## 0. 計画詳細 (2026-05-10 確定)

本 proposal は当初 alpha.10 で投入予定だった (Draft 2026-05-09) が、 dict 側の architecture 課題 (scoring-engine) を 0.1.0 stable で先に解決する判断で **0.2.0 stable target に変更**。

**version division** (確定):

| version | dict side | lib side |
|---|---|---|
| **0.1.0 stable** | bracket notation **書ける** (forward compat 受け入れ + validate 構文 check) | bracket / `/` を **strip して無視**、 reading 部分のみ使用 |
| **0.1.x patch** | bracket 付き entry 漸進蓄積 (additive) | 0.1.0 と同じ (無視継続) |
| **0.2.0 stable** | bracket 付き entry 本格活用、 `rules/accent/` 投入 | bracket parse、 `accent_phrases` field を Token に **追加** (additive)、 `--mode=accent` / `--mode=voicevox-aques` 投入、 **`tts` mode に accent 機能を追加** (削除はしない) |

**forward compat 戦略** (0.1.0 から bracket を dict 側で書ける):

- 0.1.0 dict format: `reading = "ジョ]ウズ"` のような bracket 付き reading を **許可**
- 0.1.0 lib: 読み込み時に bracket / `/` を **strip** して reading 部分のみ使用 (= accent 情報は捨てる)
- 0.1.0 validate.py: bracket 構文 check (各 phrase 内 `]` 最大 1 個 / 空 phrase 禁止 等) を入れる、 invalid bracket は parse error
- → dict contributor は 0.1.0 期間中から intonation 付き entry を書ける、 0.2.0 投入時に既に data が育ってる
- → 0.1.0 の lib 出力 (hiragana / ruby / romaji) には bracket は含まれない (strip 済)

**0.2.0 で投入する内容** (本 doc §1 以降の仕様、 ただし scoring-engine の dict format との整合チェック要):

- bracket parse 実装 (0.1.0 までの strip ロジックを置き換え)
- `Token { reading, accent_phrases: Vec<AccentPhrase> }` 内部モデル (既存 reading field 維持 + 新 field 追加 = additive、 SemVer minor 互換)
- mora 算出 (拗音 / 促音 / 撥音 / 長音 + 外来語特殊拍)
- `--mode=accent` 中立 JSON 出力 (schema_version "1")
- `--mode=voicevox-aques` AquesTalk-風記法 (仕様確定済)
- **`--mode=tts` に accent 機能を追加** (既存 pause 整形等の前処理は維持、 accent 情報を加える形で拡張、 削除はしない)
- `rules/accent/` 階層 (numbers / counters/time / prefix の seed)
- `rules/numbers/fractions.toml` (新規 rule type)

**前提条件** (0.1.0 stable で達成):

1. scoring-engine (Smart engine) が 0.1.0 stable で default 化済 ← 0.1.0 で達成
2. entry inline match による文脈依存ルビ振りが確実動作 ← 0.1.0 で達成
3. `[[kanji]]` block の架構が定着 ← 0.1.0 で達成
4. corpus regression (現 108 件) が安定 pass ← 0.1.0 で達成
5. bracket notation の dict 側 forward compat 受け入れ (validate + lib strip) ← 0.1.0 で達成
6. bracket notation を **「単語辞書 / 漢字辞書 両方の reading に opt-in 拡張」** として scoring-engine と直交する形で再設計 ← 0.2.0 で実施

scoring-engine の dict format (entry inline match / `[[kanji]]` block) と bracket notation を **直交** させる:
- bracket は reading 文字列内の opt-in markup、 scoring-engine の matcher / score logic とは別軸
- entry / kanji どちらでも `reading = "ジョ]ウズ"` のように bracket 付きで書ける
- bracket parser と scoring engine は分離、 0.2.0 で bracket parser を追加実装

---

## 1. 動機

ja-furigana は 「読み (kana) 付与」 の lookup engine だが、 TTS engine 連携時に **同形異音語 / 固有名詞 / 専門用語の accent を間違える** 問題が頻発する:

- VOICEVOX 自前 G2P で 「橋 / 箸 / 端」 を accent 区別できない
- キャラ名 / 古典固有名詞 / 通称が character-specific 読みで認識されない
- 助数詞 (3時間 / 4時間) が 数によって accent type が動的に変わる

これら 「読み付与の延長として accent 情報も提供する」 ことで、 TTS engine に対して 「読み + accent」 をワンセットで渡せるようになり、 既存の 同形異音語 / 固有名詞 誤読問題を 形態素レベルで解決する。

## 2. 設計指針

1. **既存 architecture を壊さない** — Lindera + IPADIC + ja-furigana-dict + rules の 4 層構造を維持、 各層に accent 情報を追加するだけ
2. **contributor 体験を悪くしない** — 既存 TOML 1 行 (`"surface" = "reading"`) はそのまま、 accent 必要な entry のみ bracket notation を opt-in 追加
3. **「正しさ」 を優先** — NN 予測 accent (tdmelodic 等) の bulk import は **やらない**、 手書き PR + 出典明記
4. **engine 中立 + 1 reference adapter** — 中立 token JSON (`mode=accent`) を主、 VOICEVOX 1 個だけ adapter を bundle
5. **scope を絞る** — 連濁 / 動詞活用 / 複合語 deaccenting は 0.1.0 では諦める、 逐語登録で対応

## 3. Scope (0.1.0 stable に入れる / 入れない)

### 入れる ✅

**dict 側 (ja-furigana-dict)**

- reading に bracket notation `[` `]` + accent phrase 区切り `/`
- validate.py に bracket 構文 check (各 phrase 内 `]` 最大 1 個 / mora 範囲整合 / 空 phrase 禁止)
- accent 付き entries の 出典明記 policy (NHK アクセント新辞典 / 大辞林 / 三省堂明解 等)
- `rules/accent/` 階層 新設 (numbers / counters/time / prefix の seed)
- 既存 `rules/numbers/counters/*.toml` が bracket notation を含めて OK
- `rules/numbers/fractions.toml` (新規 rule type、 `N/M` → `MブンノN`)

**lib 側 (ja-furigana)**

- reading parser が bracket `[` `]` `/` を解釈
- `Token { reading, accent_phrases: Vec<AccentPhrase> }` 内部モデル
- mora 算出 (拗音 / 促音 / 撥音 / 長音 + 外来語特殊拍 default)
- `rules/accent/` loader (空 OK、 entry あれば適用)
- 既存 counter rule loader を bracket notation 対応に拡張
- fraction composer logic (新規)
- `--mode=accent` 中立 JSON 出力 (schema_version "1")
- `--mode=voicevox-aques` AquesTalk-風記法 string 出力
- `tts` mode 完全削除 (alpha breaking change)
- CLI default mode = `hiragana`

### 入れない ❌

| 項目 | 理由 | 0.2.x 検討? |
|---|---|---|
| user_dict CSV 化 | author 体験崩壊、 over-engineering | ⚠️ 実データ次第 |
| tdmelodic 階層化 | NN 予測精度 + license + size | ⚠️ 実データ次第 |
| `voicevox-query` mode (AccentPhrase[] JSON 直生成) | VOICEVOX 仕様詳細まで踏み込む必要 | ✅ 0.2.x |
| 他 engine adapter (openjtalk / ssml / ymm4 等) | community 任せ | ❌ never (community PR 待ち) |
| engine config 外部 TOML 化 | 1 adapter のうちは過剰設計 | ⚠️ 必要が出たら |
| 連濁 accent shift | 規則性弱、 統計的 | ✅ 0.2.x |
| 動詞活用 accent | Lindera 連携必要、 重い | ✅ 0.2.x |
| 複合語一般 deaccenting | 学術論文レベルの統計処理 | ✅ 0.2.x |
| 形態素解析撤廃 (long-term) | dict 規模 200k+ 必要 | 1.0+ vision |

## 4. dict notation 仕様

### 4.1. Bracket 記法 (tdmelodic 風)

カタカナ reading 内に ASCII bracket を埋め込んで accent 核位置を示す:

| 記号 | 意味 | placement |
|---|---|---|
| `[` | 低 → 高 (上昇位置) | 上昇するモーラの **前** |
| `]` | 高 → 低 (下降位置 = アクセント核) | 下降するモーラの **前** |
| `/` | accent phrase 区切り | 句境界 |

**例**:

```toml
[entries]

# 既存形式 (accent 無し、 そのまま動く)
"切磋琢磨" = "セッサタクマ"
"枕草子"   = "マクラノソウシ"

# 0型 (平板) 明示
"霧雨"     = "キ[リサメ"
"飴"       = "ア[メ"

# 1型 (頭高)
"雨"       = "ア]メ"
"枕"       = "マ]クラ"
"猫"       = "ネ]コ"

# 中高
"桜"       = "サ[ク]ラ"

# 尾高 (末尾 `]`)
"花"       = "ハ[ナ]"
"心"       = "コ[コロ]"

# 拗音は 1 mora 扱い
"京都"     = "キョ]ウト"        # 1型
"百"       = "ヒャ[ク"           # 0型

# 複合語 (姓 + 名 = 2 phrases)
"博麗霊夢" = "ハ[クレイ/レ[イム"
"飯綱丸龍" = "イ[イズナマル/メ[グム"
```

### 4.2. policy

- **bracket 無し reading** = 「accent 不明」、 lib は `accent: null` を出力、 TTS engine 自前 G2P に委ねる
- **bracket 1 個以上ある reading** = 「accent 明示済」、 lib は bracket parse 結果に基づき accent 値を出力
- **各 accent phrase 内に `]` は最大 1 個** (東京式の 「1度下がったら戻らない」 制約)
- **`/` で複数 phrase に分けると、 各 phrase が独立した accent type を持つ**
- **TOML key (surface) の `/` は文字 literal**、 special meaning 無し
- **literal `/` を reading に書きたい場合は全角 `／` (U+FF0F)** を使う (実用上ほぼ不要)

### 4.3. mora 算出ルール

reading から bracket を除去後、 mora 単位で counting:

| パターン | mora 数え方 | 例 |
|---|---|---|
| 通常カナ | 1 mora ずつ | カ = 1 mora |
| 拗音 (`ャ` `ュ` `ョ`) | 直前と合算 | キャ = 1 mora |
| 小書き母音 (`ァ` `ィ` `ゥ` `ェ` `ォ`) | 直前と合算 (外来語) | ファ = 1 mora、 ティ = 1 mora |
| 促音 (`ッ`) | 単独 1 mora | キッ = 2 mora (キ + ッ) |
| 撥音 (`ン`) | 単独 1 mora | カン = 2 mora |
| 長音 (`ー`) | 単独 1 mora | カー = 2 mora |

**例**:

- `キョウト` = キョ(1) + ウ(2) + ト(3) = 3 mora
- `アッサリ` = ア(1) + ッ(2) + サ(3) + リ(4) = 4 mora
- `カーテン` = カ(1) + ー(2) + テ(3) + ン(4) = 4 mora

### 4.4. accent type 計算

bracket 解析後、 mora 列内の `]` 位置から accent type を計算:

| 構造 | accent type | 例 |
|---|---|---|
| `]` 無し、 `[` あり (or 無し) | 0 (平板) | `キ[リサメ` = 0、 `アメ` = 不明 (= null) |
| 1 mora 目の直後 `]` | 1 (頭高) | `ア]メ` = 1 |
| 2..N-1 mora 目の直後 `]` | M (中高) | `サ[ク]ラ` = 2、 `コ[コロ]` の `]` は末尾 → 後述 |
| 末尾 mora の直後 `]` | mora_count (尾高) | `ハ[ナ]` = 2 (mora=2)、 `コ[コロ]` = 3 (mora=3) |

## 5. lib 内部モデル

```rust
pub struct Token {
    pub surface: String,
    pub reading: String,                // bracket / `/` 除去後の純カナ
    pub accent_phrases: Vec<AccentPhrase>,
}

pub struct AccentPhrase {
    pub reading: String,                // 該当 phrase の reading
    pub mora: u8,
    pub accent: Option<u8>,             // None = 不明、 0 = 平板、 1..N = 核位置
}
```

- 既存 mode (`hiragana` / `ruby` / `romaji` / `kanji`) は `accent_phrases` を無視して `reading` の concat を出力 (= 既存挙動と同じ)
- 新 mode (`accent` / `voicevox-aques`) は `accent_phrases` を使って構造化 / 専用 format を出力

## 6. rule layer 拡張

### 6.1. 既存 counter rule (`rules/numbers/counters/*.toml`)

`default` / `specials` / `replacements` / `rules` の reading 値に bracket notation を載せられるよう拡張:

```toml
# rules/numbers/counters/time.toml (拡張例)
[counter."時"]
default = "ジ]"                                 # 1型 (1ジ)
specials = { "0" = "レ[イジ", "4" = "ヨ]ジ", "7" = "シ[チジ", "9" = "ク]ジ" }

[counter."時間"]
default = "ジ]カン"
specials = { "3" = "サ[ンジカン", "6" = "ロ[クジカン", "8" = "ハ[チジカン" }
```

contributor は 既存 entry そのままで OK、 accent 付与は地道に PR で。

### 6.2. 新規 `rules/accent/` 階層

dict と独立した accent helper rules:

```
rules/accent/
  _genre.toml
  numbers.toml              (1〜10 の数字単独 accent)
  counters/
    _genre.toml
    time.toml               (時間系の number×counter accent table)
  prefix.toml               (お / ご / 夜 等の接頭辞 accent)
```

**rules/accent/numbers.toml** (seed):

```toml
[meta]
role = "accent_numbers"
description = "数字単独のアクセント (1〜10、 NHK 準拠)"

[entries]
"0" = "レ]イ"
"1" = "イ]チ"
"2" = "ニ]"
"3" = "サ]ン"
"4" = "ヨ]ン"
"5" = "ゴ]"
"6" = "ロ]ク"
"7" = "ナ]ナ"
"8" = "ハ]チ"
"9" = "キュ]ウ"
"10" = "ジュ]ウ"
```

### 6.3. 新規 `rules/numbers/fractions.toml`

```toml
[meta]
role = "fractions"
description = "分数 (N/M → MブンノN、 動的 accent 合成)"

[fraction]
pattern = '(\d+)\s*/\s*(\d+)'
template = "{denom_kana}ブンノ{num_kana}"
# {denom_kana} / {num_kana} は rules/accent/numbers.toml の bracket 付き reading が展開される
# accent: 連結時に 「ブンノ」 部分は平板継続、 末尾 phrase に accent shift
```

**例**: `2/4` の処理
1. parse: numerator=2, denominator=4
2. lookup: numerator → `ニ]`、 denominator → `ヨ]ン`
3. compose: `ヨ]ンブンノ/ニ]` (各数字は phrase 区切り、 元の accent 維持)
4. output: 2 phrases、 1型 + 1型

(細部は実装時に調整、 「ブンノ」 部分の accent は別途仕様確定要)

## 7. 出力 mode

### 7.1. `--mode=accent` (中立 JSON)

```json
{
  "schema_version": "1",
  "tokens": [
    {
      "surface": "雨",
      "reading": "アメ",
      "accent_phrases": [
        {"reading": "アメ", "mora": 2, "accent": 1}
      ]
    },
    {
      "surface": "が",
      "reading": "ガ",
      "accent_phrases": [
        {"reading": "ガ", "mora": 1, "accent": null}
      ]
    },
    {
      "surface": "降る",
      "reading": "フル",
      "accent_phrases": [
        {"reading": "フル", "mora": 2, "accent": null}
      ]
    }
  ]
}
```

- `accent: null` = 不明 (TTS engine 自前 G2P に委ねる)
- `accent: 0..N` = 明示 (0=平板、 1..N=核位置)
- `schema_version` で future-proof、 新 field 追加に対応

### 7.2. `--mode=voicevox-aques`

VOICEVOX AquesTalk-風記法 (kana 記法) に変換。
仕様 reference: [voicevox_engine/tts_pipeline/kana_converter.py](https://github.com/VOICEVOX/voicevox_engine/blob/master/voicevox_engine/tts_pipeline/kana_converter.py)

```bash
furigana --mode voicevox-aques "雨が降る"
# 出力: ア'メガ/フル'
```

#### 仕様確定事項 (ソース確認済)

1. **`'` placement**: 核モーラの **直後** に配置 (1-indexed)
   - `ア'メ` = accent=1 (頭高)
   - `アメ'` = accent=2 (= mora count)

2. **平板 (0型) の表記** ★ 重要な制約 ★
   - VOICEVOX kana 記法は **accent=0 を表現できない** (`'` 省略はエラー)
   - **平板 entry は 末尾 `'` で出力する** (= accent_index = mora_count)
   - VOICEVOX 内部表現的には 「平板」 と 「尾高」 が kana 記法上で区別不可、 両者とも末尾 `'`
   - 差は次 phrase / 文末 particle 付き時のみ顕在化、 単独発音では同じ

3. **句区切り**:
   - `/` = silence なし区切り (default)
   - `、` = silence あり区切り (breath 入れたい時のみ、 0.1.0 では出力しない)

4. **疑問文**: 全角 `？` のみ、 句末固定 (0.1.0 では入力 surface に `？` あれば passthrough)

5. **無声化 `_`**: ja-furigana 側では出力しない、 VOICEVOX 自前推定に委ねる

6. **カナ正規化**: カタカナ出力 (ひらがな / 半角カナは VOICEVOX 側で reject される可能性)

#### エラー条件 (回避すべきパターン)

| 条件 | 回避策 |
|---|---|
| `'` 完全省略 (`ACCENT_NOTFOUND`) | accent_phrases の各 phrase で必ず `'` 出力 |
| 1 phrase 内 `'` 2 個以上 (`ACCENT_TWICE`) | 1 phrase = 1 accent type の規約守る |
| 句頭 `'` (`ACCENT_TOP`) | accent=0 を末尾 `'` 出力に変換、 accent>=1 は核モーラの後に |
| 空 phrase (`/` 連続) (`EMPTY_PHRASE`) | 区切り間に必ず内容を出す |
| 1 phrase の長さ > 300 文字 | 通常の token 単位なら問題なし |

#### 変換 logic (lib 内部)

```rust
// AccentPhrase { reading: "アメ", mora: 2, accent: Some(0) }
// → "アメ'"  (平板 = mora count 末尾 `'`)

// AccentPhrase { reading: "アメ", mora: 2, accent: Some(1) }
// → "ア'メ"  (頭高 = 1 mora 目の後 `'`)

// AccentPhrase { reading: "サクラ", mora: 3, accent: Some(2) }
// → "サク'ラ"  (中高 = 2 mora 目の後 `'`)

// AccentPhrase { reading: "ハナ", mora: 2, accent: Some(2) }
// → "ハナ'"  (尾高 = 末尾 `'`、 平板と同形式)

// AccentPhrase { reading: "ガ", mora: 1, accent: None }
// → 平板扱いで "ガ'" 出力 (accent 不明なら平板で fallback)
//   or 直前 phrase に attach して 助詞 として扱う (要検討)
```

`accent: None` の扱いは 0.1.0 投入前に確定:
- **案 A**: 平板 fallback (`ガ'` 出力)、 VOICEVOX が validate 通る
- **案 B**: 直前 phrase の末尾に kana 連結 (= 助詞 attach)、 VOICEVOX 流の accent phrase になる
- 案 B が VOICEVOX 慣例だが実装コスト高、 0.1.0 では案 A から開始

## 8. `tts` mode に accent 機能を追加 (削除しない)

**方針変更 (2026-05-10)**: 旧 proposal では `tts` mode を削除して `--mode=accent` / `--mode=voicevox-aques` に置き換える計画だったが、 **既存 `tts` mode を維持して accent 機能を追加** する方針に変更。

### 既存 `tts` mode (維持)

```rust
let f = Furigana::default();
let opts = TtsOptions::default();
let s = f.to_tts("こんにちは。さようなら、また。", &opts);
// → "こんにちは。 さようなら、 また。"   (既存挙動: pause 整形)
```

`normalize_for_tts` / `segment_for_tts` / `TtsOptions` 等の API は **継続して提供**。

### 0.2.0 での拡張 (additive)

`tts` mode の出力に **accent 情報を埋め込む** 形で拡張、 既存挙動は default 維持:

```rust
// default (既存挙動と同じ、 accent なし)
let s = f.to_tts(text, &opts);

// accent 込み (新オプション、 0.2.0 で追加)
let opts = TtsOptions { include_accent: true, ..Default::default() };
let s = f.to_tts(text, &opts);
// → 何らかの accent marker 付き文字列、 詳細仕様は 0.2.0 で確定
```

具体的な accent embedding 仕様 (例: AquesTalk-風 `'` を tts mode 出力に統合する / 別 sub-mode を切る 等) は 0.2.0 着手時に確定。

### 新 mode との関係

- `--mode=tts`: pause 整形 + (0.2.0 から) accent 機能 opt-in
- `--mode=accent`: 中立 JSON 出力 (0.2.0 新規、 構造化データ用途)
- `--mode=voicevox-aques`: AquesTalk-風記法 string (0.2.0 新規、 VOICEVOX 専用)

3 つの mode は併存、 用途で使い分け。 削除なし。

## 9. 長期 vision

dict が 50k → 200k → 500k と育つにつれ、 Lindera + IPADIC の付加価値は逓減する。 1.0+ では:

```
入力 → ja-furigana-dict (longest-match + 活用 rule) → 出力
        ↑ pure Rust、 deps 最小、 軽量、 license clean
```

の純粋 dict-driven 路線を検討。 0.1.0 stable で固める accent annotation の流儀は、 この方向と整合する (accent は dict TOML、 user_dict CSV ではない)。

詳細は ROADMAP.md 参照。

## 10. 未確定 / 未解決事項

1. ~~**VOICEVOX 平板 `'` placement**~~: ✅ 確定 (§7.2 参照、 末尾 `'` で出力 = accent_index = mora count)
2. **fraction の 「ブンノ」 部分の accent**: 平板継続? 句区切り独立? 出典要 (実装時に確定)
3. **数字 accent (rules/accent/numbers.toml seed)**: 出典 = NHK アクセント新辞典 を first source とする、 入手次第確定
4. **接頭辞 rule (お / ご / 夜)**: 例外 case (「夜中」 等) の扱い、 個別 entry override で吸収する流儀で OK か (実装後 dogfood で判断)
5. **JSON schema_version "1"**: 0.2.x で field 追加時に bump するかしないか policy (additive な field 追加なら bump しない、 destructive な変更で bump、 で OK)
6. **`accent_phrases` 配列が空の token**: 助詞 / 句読点 / 記号等は phrases 0 個出すか 1 個出すか
7. **VOICEVOX adapter で `accent: None` の token 扱い**: 平板 fallback (案 A) か助詞 attach (案 B) か、 0.1.0 は案 A で開始
8. **拗音 / 撥音 / 促音 / 長音 の mora カウント**: VOICEVOX 内部辞書に依存、 ja-furigana 側で個別 verify するか TTS 側に委ねるか — 0.1.0 では我々の標準ルール (§4.3) で出力、 不一致が出たら個別 entry で対応

## 11. References

- [tdmelodic 公式 doc](https://tdmelodic.readthedocs.io/ja/latest/) — bracket notation の参考 (記法のみ拝借、 dict は使わない)
- [VOICEVOX](https://github.com/VOICEVOX/voicevox_engine) — AquesTalk-風記法、 仕様確認中
- [NHK アクセント新辞典](https://www.nhk-book.co.jp/detail/000000950462015.html) — 出典として最も信頼性高
- [モーラ Wikipedia](https://ja.wikipedia.org/wiki/%E3%83%A2%E3%83%BC%E3%83%A9) — mora 数え方の標準

---

**次のアクション**:
1. VOICEVOX 仕様 fork report 取り込み (本書 §7.2 / §10 を確定)
2. alpha.9 タスク細分化
3. 実装着手 (別 session)
