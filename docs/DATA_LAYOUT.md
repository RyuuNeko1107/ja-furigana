# データ配置

辞書 / ルール TOML の置き場所と探索順。

> 戻る: [README](../README.md)
> 関連: 各 file の TOML schema は [`ja-furigana-dict/docs/SCHEMA.md`](https://github.com/RyuuNeko1107/ja-furigana-dict/blob/master/docs/SCHEMA.md) /
> 辞書 merge 順 + reading 解決順は [ARCHITECTURE.md](./ARCHITECTURE.md#step-5-の詳細-resolve_reading-の-6-段階優先順位) /
> 設定全項目は [CONFIG.md](./CONFIG.md)

## 既定の場所 (portable 配置)

default では **実行ファイルと同じディレクトリ**:

```
<furigana.exe と同じフォルダ>/
├── furigana.exe                   # 本体 (Windows なら .exe / 他は単に furigana)
├── config.toml                    # 設定 (任意)
├── repl_history                   # REPL の入力履歴 (自動生成)
└── data/                          # `furigana dict pull` で展開、ユーザー追加もここに集約
    ├── unihan/*.toml              # 単漢字フォールバック (5 水準別、 43k+ 字)
    ├── kanji/*.toml                # ★A2 alpha.11 単漢字 [[kanji]] block (default + 文脈分岐 reading)
    ├── compat.toml                # 異体字マップ (髙→高 等)
    ├── jukugo/                    # 熟語 / 固有名詞 / 文化系 (24 ファイル、カテゴリ別分割、 文脈分岐は entry inline match)
    ├── works/                     # 作品単位辞書 (任意深度のサブディレクトリ)
    │   └── game/touhou.toml       #   例: 東方Project (公式読みのみ採録、 出典コメント必須)
    ├── loanwords/                 # 外来語 (IT 用語等の英字 surface 専用、 別 lookup 経路)
    │   └── it.toml                #   例: Kubernetes / Docker / TypeScript / PostgreSQL …
    ├── days.toml                  # 1〜31 日特殊読み
    ├── scales.toml                # 万 / 億 / 兆 / 京 / 垓 ...
    ├── units.toml                 # SI 単位 + 「円」「%」
    ├── symbols.toml               # 記号読み (〜→から 等)
    ├── emoji.toml                 # 絵文字の読み (名詞は文中で使われた時だけ読む、 空 = 読まない、 麻雀牌は常に読む)
    ├── emoji_context.toml         # 絵文字の文脈読み ([[entries."X".match]]: 直前 / 直後 / 文中の語 / 発言がその絵文字だけ)
    ├── models.toml                # 型番パターン (正規表現 + 雛形。 Ryzen 7 5800X3D / RTX 4070 Ti / GPT-4o 等)
    ├── context_cues.toml          # N/M が日付か分数かを決める手がかりの語、 伏せ字の敬称
    ├── numeric_phrases.toml       # 慣用語句 (二十歳→ハタチ 等)
    ├── counters/*.toml            # 助数詞ルール (年度 / 時間半 含む 7+ ファイル)
    ├── postprocess.toml           # 後処理 regex 置換
    ├── user/                      # ユーザー追加 (`furigana dict add` で自動生成)
    │   └── cli-added.toml
    └── overrides.toml              # 強制上書き用 (最優先、任意)

# ★A2 alpha.11 で削除: single_overrides.toml (= core/kanji/overrides.toml に migrate) /
#                       latin.toml (= dead code、 alpha.7 loanwords 投入で経路廃止) /
#                       context/*.toml (= entry inline match に migrate)
```

zip / tar.gz を解凍したフォルダごとコピーすれば持ち運べる **portable 配置**。

## カスタム場所 + 環境変数

CLI flag および環境変数で `<data_dir>` を変えられる:

```sh
# CLI flag
furigana lookup '灰桜' --data-dir /var/lib/furigana

# 環境変数 (優先度: CLI flag > env)
export FURIGANA_DATA_DIR=/var/lib/furigana
furigana lookup '灰桜'
```

`current_exe()` の解決に失敗した稀なケースのみ XDG fallback:
- Linux: `~/.local/share/furigana/`
- Windows: `%LOCALAPPDATA%\furigana\furigana\`
- macOS: `~/Library/Application Support/com.furigana.furigana/`

## 辞書ソースの merge 順

`Furigana::builder` で組立、 後勝ち:

1. **`core_dict_dir`** ← 配布版 (`furigana dict pull`)、 最弱
2. **`user_dict_dir`** ← `furigana dict add` の保存先 (`cli-added.toml`)
3. **`overrides_file`** ← `data/overrides.toml`
4. **`add_entry`** ← API で直接追加 (最強)

その上で **chunks/split() の階層** や **token 単位の reading 解決順** が動く。
詳細は [ARCHITECTURE.md](./ARCHITECTURE.md) の Step 2 / Step 5 解説を参照。

`furigana dict add 灰桜 ハイザクラ` で `data/user/cli-added.toml` に追加 → 次回起動時 (or `:reload`) で反映。

## `furigana dict pull` の動き

GitHub Releases から `ja-furigana-dict` の tarball を取得 + SHA-256 検証 + 展開する。

```sh
furigana dict pull                           # 最新 release
furigana dict pull --version v2026.05.07     # version pin
furigana dict pull --allow-unverified        # SHA-256 sidecar が無い release を、 検証なしで取る (既定は中止)
```

サーバ運用 (alpha.5+) では、 これを **起動時に自動実行** する `--auto-pull` フラグや、
**起動中に定期 polling** する `[auto_update]` config もある (admin_tokens 不要)。
詳細は [HTTP_API.md](./HTTP_API.md#ホットリロード--自動更新) を参照。

詳細フロー:

1. `--version` 指定 or GitHub API `/releases/latest` で tag 解決 (tag 文字列は strict format validate で reject 防御)
2. `furigana-dict-{tag}.tar.gz` をダウンロード (50 MB 上限)
3. `furigana-dict-{tag}.tar.gz.sha256` (sidecar) を取得して **SHA-256 検証**。 sidecar が取れなければ **中止** する
   (`--allow-unverified` を付けた時だけ検証なしで続行)。 sidecar は tarball と同じ配布元から取るので、 検知できるのは破損であって
   改ざんではない点に注意
4. 一時ディレクトリ (`<data_dir>/.data-new-*`) へ tarball を展開 (200 MB 総 cap / 1 entry 10 MB (PAX 拡張で宣言したサイズも含む) /
   50,000 entries / Regular file + Directory のみ許可) + path rebase: archive 内の `core/X` / `rules/X` を `X` に flat 化。
   archive 内の `user/` と `overrides.toml` は **無視する** (配布物でユーザーのデータを上書きさせない)
5. 旧辞書の **`user/` と `overrides.toml` を引き継ぎ**、 `<data_dir>/data/` と rename で入れ替える。 途中で失敗しても旧辞書は残る
   (`<data_dir>/.furigana-dict.lock` で、 同時に 2 つの pull が走らないよう排他)

archive 側 (`ja-furigana-dict` repo) は `core/` `rules/` の 2 階層で PR レビュー上の分類のために維持されている。 配布物だけ flat に変換される。

```
archive 内 (PR レビュー用)              →   利用者の <data_dir>/data/ (flat-ish)
core/<...>.toml                        →   data/<...>.toml         (core/ prefix を剥がす)
rules/<...>.toml                       →   data/<...>.toml         (rules/ prefix を剥がす)
```

具体的な階層構造 (genre dir / 水準別分割 / 作品 dir 等) は ja-furigana-dict 側で進化するので、
`Dict::from_toml_dir` の **無制限階層再帰** で吸収する設計 (lib 側に階層仮定なし)。
`_genre.toml` のような sub-section description ファイルは lib では silent skip + release tar
から `--exclude='_genre.toml'` で除外されるため、 利用者の `data/` 配下には届かない。

path traversal 防御として、 archive entry の path は **ディレクトリを作る前に** 構成要素を検査する
(`..` / 絶対パス / ドライブ名 / `:` を含む名前は拒否)。 展開先は新規の一時ディレクトリ (symlink は作られない) なので、
`<data_dir>/data/` の外には何も書かれない。

## REPL からの操作

```
all> :pull              # 同名コマンド (`furigana dict pull` 相当)
all> :pull v2026.05.07   # version pin
all> :reload            # data_dir から in-memory 辞書を再 build
all> :size              # dict_size 表示
```

詳細は `furigana repl` 起動後 `:help` で。
