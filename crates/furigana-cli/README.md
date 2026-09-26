# ja-furigana-cli

`furigana` CLI バイナリ + ローカル HTTP サーバー。
[`ja-furigana`](https://crates.io/crates/ja-furigana) lib crate のフロントエンド。

> Status: 0.5.x (crates.io 公開中、 変更履歴は [CHANGELOG](https://github.com/RyuuNeko1107/ja-furigana/blob/master/CHANGELOG.md))。

## インストール

```sh
cargo install ja-furigana-cli
# → ~/.cargo/bin/furigana がインストールされる
```

GitHub Releases から OS 別の binary をダウンロードする方法もあります
([RyuuNeko1107/ja-furigana/releases](https://github.com/RyuuNeko1107/ja-furigana/releases))。

## 使い方

```sh
# 1 ショット変換
#   --mode: tts (default) | hiragana | ruby | kanji | romaji | romaji-kunrei |
#           analyze | accent | voicevox-aques | aquestalk
#   alias : bouyomi = tts / hira = hiragana / kunrei = romaji-kunrei / voicevox = voicevox-aques
furigana lookup '灰桜の散る道'                       # → tts (default)
furigana lookup '灰桜の散る道' --mode ruby           # → {灰桜|はいざくら}...
furigana lookup '灰桜の散る道' --mode hiragana       # → はいざくらのちるみち
furigana lookup '灰桜の散る道' --mode romaji         # → haizakura no chiru michi (ヘボン式)
furigana lookup '灰桜の散る道' --mode romaji-kunrei  # → 訓令式

# 出力ルール: 漢字 → ひらがな化、 アルファベット / 数字 / 記号 → カタカナ統一
furigana lookup 'Anthropic の Claude を使う' --mode hiragana
# → アンソロピックのクロードをつかう
furigana lookup 'PostgreSQL 16 で動かす' --mode hiragana
# → ポストグレスキューエルジュウロクでうごかす

# TTS エンジン向け
furigana lookup '峠道に' --mode voicevox-aques       # VOICEVOX の kana 記法 (is_kana=true 用)
furigana lookup '峠道に' --mode aquestalk            # 本家 AquesTalk の音声記号列
furigana lookup '長い文…' --mode aquestalk --max-len 255   # 長さ上限で分割、 1 行 1 塊
furigana lookup '峠道に' --mode accent --estimate-accent   # accent JSON (dict に無い語も rule で推定)

# 主なオプション
#   --silence-symbols   tts: 絵文字 / 顔文字パーツを読み上げから外す
#   --drop-period       tts/aquestalk: 末尾の 。 を残さない
#   --no-devoice        aquestalk: 無声化記号 _ を付けない
#   --batch             stdin を 1 行 1 入力で読み 1 行 1 結果を出す (辞書 load 1 回)
printf '灰桜
散る道
' | furigana lookup --batch --mode hiragana

# 対話モード (REPL) — 引数なしで起動 = REPL (Windows なら exe ダブルクリック相当)
furigana
furigana repl --mode hiragana

# 辞書管理
furigana dict pull                       # GitHub Release から最新 furigana-dict を取得
furigana dict pull --version v2026.09.25  # version pin (dict は日付 tag)
furigana dict add 灰桜 ハイザクラ        # ユーザー辞書に追加
furigana dict list                       # 現状サマリ
furigana dict remove 灰桜
furigana dict import path/to/extra.toml  # 既存 TOML を user 配下に取り込み

# ローカル HTTP サーバー (`/furigana` エンドポイント)
furigana serve                                 # http://127.0.0.1:8000
furigana serve --bind 0.0.0.0:8000             # 外部からも叩く
furigana serve --auto-pull                     # 起動時に最新 dict を自動取得
furigana serve --estimate-accent               # accent 系 mode で rule-based accent 推定
FURIGANA_TOKEN=<secret> furigana serve         # 認証有効
```

辞書を最新化する一番シンプルな方法は **`furigana dict pull` してから process を再起動**。
無停止運用したい場合は `--auto-pull` (起動時 1 回) や `[auto_update]` 定期 polling
(config.toml に 1 セクション、admin_tokens 不要) が選択肢。詳細は
[`docs/HTTP_API.md`](https://github.com/RyuuNeko1107/ja-furigana/blob/master/docs/HTTP_API.md#ホットリロード--自動更新) を参照。

データディレクトリの default は **実行ファイルと同じフォルダ** (portable 配置)。
`--data-dir <path>` または `FURIGANA_DATA_DIR` で上書き可能。

詳細は [プロジェクト README](https://github.com/RyuuNeko1107/ja-furigana) を参照。

## ライセンス

MIT License.
