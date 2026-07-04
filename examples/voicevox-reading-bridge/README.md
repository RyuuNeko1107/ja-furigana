# VOICEVOX reading bridge (サンプル)

ja-furigana で**誤読した単語だけ**読みを直して VOICEVOX に喋らせる、
棒読みちゃん互換のローカル読み上げサーバーのサンプル実装。

コメント読み上げ (わんコメ等の棒読みちゃん連携) をそのまま VOICEVOX +
ja-furigana の読み精度に乗せ換えられる。

## 設計: 韻律はエンジン、読みは辞書

TTS エンジンの抑揚は**漢字のまま渡した時が最も自然**になる。

| 渡し方 | 読み | 抑揚 |
|---|---|---|
| 漢字のまま | エンジン次第 (時々誤読) | ◎ |
| 全文ひらがな | 正しい | △ 平坦になる |
| アクセント記法で全上書き | 正しい | × 句が細切れ |
| **本ブリッジ (誤読単語のみカナ)** | **正しい** | **◎ (犠牲は誤読語のみ)** |

節ごとに ja-furigana と VOICEVOX 自身の読みを phoneme 列で比較し、
ズレた区間に重なる単語だけ読みカナに置換する:

```
入力:   今日は大変いい天気ですね峠道は険しいですが
VOICEVOX: きょうは…「とうげどう」…   ← 誤読
出力:   今日は大変いい天気ですね「とうげみち」は険しいですが
        (峠道だけカナ化、他は漢字のままエンジンの韻律をフル活用)
```

## 使い方

```sh
# 1. ja-furigana server (要: cargo install ja-furigana-cli など)
furigana dict pull      # 辞書取得 (初回のみ)
furigana serve          # localhost:8000

# 2. VOICEVOX を起動 (localhost:50021)

# 3. 本ブリッジ (Node.js 18+、npm install 不要)
node bridge.js          # localhost:50080

# 4. 棒読みちゃん互換クライアントを localhost:50080 に向ける
#    (動作確認: curl "http://localhost:50080/talk?text=峠道に注意")
```

## 環境変数 (全部 optional)

| 変数 | default | 意味 |
|---|---|---|
| `BRIDGE_PORT` | 50080 | 待受 port (棒読みちゃん互換) |
| `FURIGANA_API` | http://127.0.0.1:8000/furigana | ja-furigana endpoint |
| `FURIGANA_API_KEY` | (なし) | serve を認証付きで立てた場合の X-API-Key |
| `VOICEVOX_URL` | http://127.0.0.1:50021 | VOICEVOX ENGINE |
| `VOICEVOX_SPEAKER` | 3 (ずんだもん ノーマル) | style id |
| `API_TIMEOUT_MS` | 1500 | API timeout (超過で素通し fallback) |
| `READING_FIX` | 1 | 0 で読み修正を切って完全素通し |

## 実装メモ (ハマりどころ)

- **漢字を含まない節は読み修正しない**: かな文は直すものが無い上、 助詞
  は/へ/を の発音差 (ハ vs ワ) で偽陽性が出て逆に改悪する
- **単語単体でエンジンに読みを聞き直す方式は不可**: エンジンは 「峠道」 単体なら
  正読するのに 「峠道は」 で誤読する等、 文脈で分かち書きと読みが変わる →
  文全体の phoneme 列同士を LCS diff してズレた単語を特定する
- **表記読みと発音カナの差** (トウゲ vs トオゲ、 カンリョウ vs カンリョオ) は
  phoneme 化の際に長音へ畳んで吸収する
- 壊れても喋る: API 失敗 → 漢字素通し / VOICEVOX 停止 → log だけ出して skip
- `mismatch_log.jsonl` に不一致が貯まる = エンジンと辞書のどちらかが誤読した
  証拠なので、 辞書改善の材料になる

## 対応 endpoint

`/talk` (本体) / `/clear` / `/skip` / `/getTalkTaskCount` + 互換 no-op stub。
レスポンスは `{"taskId":n}`。

再生は PowerShell SoundPlayer (Windows 前提)。 他 OS では `playWav` を
`aplay` / `afplay` 等に差し替えれば動く。
