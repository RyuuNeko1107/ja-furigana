# 規則の表を辞書 (データ) 側で直せるようにする

状態: **方針決定済み・実装前**。 最初の対象は人名の手がかり (敬称・肩書き)。

## 1. 目的

誤読の修正には 2 種類ある。

| 種類 | 直す場所 | 反映までの手間 |
|---|---|---|
| 単語の読み | 辞書 (`furigana-dict`) | `dict pull` だけ。 lib のリリース不要 |
| **規則に使う表** (敬称・肩書き・関数名・月の読み …) | lib の Rust ソース | lib のリリースと crates.io 公開、 利用側の更新が要る |

後者を辞書側に出し、 規則の表も **辞書の更新だけで直せる** ようにする。 Rust には、
アルゴリズムの定数だけを残す。

## 2. すでにある前例

`context_cues.toml` (`crates/furigana/src/rules/context_cues.rs`) が同じことをしている。
`N/M` が日付か分数かの手がかり語を、 辞書側から **足せる**。 読み込み (`loader.rs` の role 判定)、
複数ファイルの合算 (`merge`)、 空の語の検査 (`validate`) の道筋がそろっているので、 これに倣う。

## 3. 棚卸し (Rust に固定されている表)

「辞書へ」 は、 直す頻度が高く、 値が語彙の性質のもの。 「Rust に残す」 は、 アルゴリズムの定数。

| 場所 | 表 | 方針 |
|---|---|---|
| `person_names/cues.rs` | `HONORIFICS` (さん・ちゃん・くん・氏)、 `TITLES` (選手・監督 … 約 50)、 `COMMON_WORD_GIVEN` (新人・初代・勝利)、 肩書きの後で複合語になる頭の字 (会・団・連 …) | **辞書へ (第 1 弾)** |
| `person_names/cues.rs` | `MAX_RUN_CHARS` / `MAX_RUN_TOKENS` / `ALT_WEIGHT` | Rust に残す (アルゴリズムの定数) |
| `numbers/context.rs` | `BUILTIN_HONORIFICS` (伏せ字の敬称)、 `DECORATIVE_SHAPES` (●■▲) | 辞書へ。 敬称は `context_cues.toml` の `[placeholder]` に既にある (足すだけ) |
| `math/chars.rs` | `FUNCS` (sin・cos …)、 ギリシャ文字、 `UNIT_RUNS`、 `WORDS` | 辞書へ (第 2 弾) |
| `chain_phrases/` | `MONTH_READINGS`、 `LETTER_NAMES` (英字の名前)、 `COMPOUND_POS` | 月の読みは辞書へ (第 2 弾)。 品詞の対応は Rust に残す |

## 4. 方式

### 4-1. 置き場所と形式

`rules/person_name_cues.toml` (role = `person_name_cues`)。 `context_cues.toml` と同じ流儀。

```toml
[meta]
role = "person_name_cues"
schema_version = "2"

[cues]
honorifics        = ["さん", "ちゃん", "くん", "氏"]
titles            = ["選手", "監督", "先生", "..."]
not_given_names   = ["新人", "初代", "勝利"]          # 名の表にあっても名として扱わない語
compound_heads    = ["会", "団", "連", "協", "組", "学", "院", "所"]  # 肩書き + これで始まる語は別の複合語
```

### 4-2. 辞書が正 (Rust に既定値は持たない) — 決定済み

辞書は必須の前提なので、 **手がかりの表は辞書のファイルだけを正にする**。 Rust には既定値を残さない。

| 状態 | 動き |
|---|---|
| `person_name_cues.toml` が **ある** | その内容を使う。 複数ファイルは `merge` で合算 |
| **ない** | 手がかりが空 = 人名の専用経路は何もしない (名の表も辞書にあるので、 辞書が無い環境では元から動かない) |

`context_cues.toml` は 「足すだけ」 だが、 手がかりは **削る** 修正 (例: 勝利を名として扱わない) が
要るので、 辞書側の内容を丸ごと使う。

**切り替えの順序** (安全のため):

1. 先に **辞書** へ `person_name_cues.toml` を足して配布する。 古い lib は未知の role を読み飛ばす
   (`load_rules_dir` の `_ => {}`) ので壊れない。 古い lib は自前の既定値で動き続ける
2. 次に **lib** を、 Rust の既定値を消した版にして出す。 新しい lib は辞書のファイルを読む

辞書が先に行き渡っていない環境で新しい lib を使うと、 手がかりが空になり人名の経路が止まる。
そのため lib の変更履歴 (CHANGELOG) と `docs/CONFIG.md` に 「`person_name_cues.toml` を含む辞書が必要」 と書く。

### 4-3. lib 側の変更

- `rules/person_name_cues.rs` に `PersonNameCuesData` (読み込み・`merge`・`validate`) を追加し、
  `RulesData` に載せる
- `PersonNamePass` が持つ手がかりを、 定数から **構築時に作る表** (`PersonCues`) に替える。
  前判定の字の集合 (今は `once_cell` の静的変数) も、 この表の構築時に 1 度だけ作る。 解析ごとには作らない
- `token_may_be_cue` が特別扱いしている `ん` / `ゃん` (「さ|ん」「ちゃ|ん」のように敬称が token をまたぐ形) は、
  敬称の表から導く
- 配線: `Furigana` の構築時に表を作り、 `Pipeline` → `postpass::apply_all` に渡す

### 4-4. 検査と上限

辞書由来の値なので、 これまでの監査と同じ水準で縛る。

- 空・空白だけ・制御文字・bidi 制御は拒否 (`sanitize.rs` を通す)
- 1 語 16 字まで、 1 つの表は 1,000 語まで。 超えたら読み込みを拒否する
- 重複は除く。 並びは決定的にそろえる (実行ごとに結果が変わらない)

### 4-5. 辞書 repo 側

- `docs/SCHEMA.md` に schema を書く
- `tools/validate.py` が書式 (空語・長さ・件数) を検査する
- 最初のファイルに現在の値を移す。 別 PR (lib の PR のマージ後)

## 5. 検証

- 辞書に **今の値をそのまま** 書いたファイルを置いたとき、 辞書 repo の corpus 14,789 ケースの出力が、 変更前の lib と **完全一致**
- 辞書で 1 語を削る / 足すと、 その分だけ出力が変わるテスト
- ファイルが無いときは手がかりが空になり、 人名の経路が何もしないテスト
- 速度: 手がかりの前判定は人名の経路の入口。 構築時に表を作る形にして、 解析時の仕事は今と同じにする。 前後で中央値を比べる

## 6. 手順 (PR を分ける。 順序が大事)

1. **dict**: schema 文書、 `validate.py` の検査、 今の値のファイル `person_name_cues.toml`。 先に配布する
2. **lib**: `PersonNameCuesData` と配線、 Rust の既定値の削除、 テスト、 `docs/CONFIG.md` に schema。
   fixture には今の値のファイルを置く
3. 以降、 第 2 弾 (数式の関数名、 月の読み) を同じ形で

## 7. 決定

1. **置き換え方式**にする (4-2)
2. **Rust の既定値は持たない** (辞書が必須の前提)。 辞書を先に出し、 その後に lib を出す
3. 第 2 弾以降の優先順位 は、 第 1 弾の後に決める

## 8. 実装の前提

ファイル分割の PR (#112〜#120) のマージ後に始める (`person_names/cues.rs` が分割後のパスのため)。
