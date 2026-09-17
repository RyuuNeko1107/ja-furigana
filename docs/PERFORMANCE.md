# エンジン性能 (furigana 0.4.3)

実測日: 2026-09-17 (lib latency は最適化後に再計測) / 実測環境: Windows 11 Home (10.0.26200)、 release build、
dict = ja-furigana-dict `1f3f5105` (jukugo 22,936 + unihan 43,446 = **66,382 entries**)

計測対象は CLI (`furigana.exe lookup --mode hiragana --batch`)。 入力は実コメント
コーパス (stream-comments 収集分、 1 行 = 1 コメント、 平均 32 B/行)。

## 1. バッチスループット (CLI 実測)

| 入力 | バイト数 | 実時間 | 起動を除いた処理時間 | スループット |
|---|---|---|---|---|
| 1 行 | 30 B | 0.49〜0.60 s | — | (起動 + 辞書ロードの切片) |
| 10,000 行 | 312 KB | 0.831 s | 0.33 s | **30,300 行/s** / 0.95 MB/s |
| 50,000 行 | 1.61 MB | 1.553 s | 1.05 s | **47,600 行/s** / 1.53 MB/s |
| 200,000 行 | 6.44 MB | 6.06〜6.27 s | 5.57 s | **35,900 行/s** / 1.16 MB/s |

- **起動 + 辞書ロード = 約 0.50 s** (1 行実行を 3 回測った下限 0.492 s)。
  66k entry の TOML パースと IPADIC 読み込みを含む固定コスト。
- 1 行あたり実効コスト = **21〜33 µs** (= 処理時間 / 行数)。 行長に依存し、
  50k 行セット (平均 32 B) が最速、 200k 行セットは長い行が混ざるため低下。
- 200k 行 (6.4 MB) を **約 6 秒**で全処理。 収集コーパス 790 万行の
  フルスキャンは単一プロセスで **約 4 分**の計算。

### 起動コストの意味

バッチ処理では 20 万行でも固定 0.5 s は全体の 8% にすぎないが、
1 行だけ処理する用途では **99% が起動コスト**。 常駐 (HTTP serve) 前提の
設計が効く領域で、 CLI 単発呼び出しをループで回すのは避けること。

## 2. ライブラリ単体 latency (criterion、 CHANGELOG より)

2026-09-17 実測 (criterion、 Windows 11、 実辞書 66,382 entry mount、 IPADIC default)。
プロセス起動を含まない純粋な API latency。 bucket 絞り込み最適化 (§3) の前後:

| input | `to_ruby()` 最適化前 | `to_ruby()` 最適化後 | 変化 |
|---|---|---|---|
| short (18 B) | 11.5 µs | **8.9 µs** | -22% |
| short_phrase (27 B) | 24.5 µs | **14.2 µs** | -43% |
| medium (110 B) | 85.3 µs | **54.6 µs** | -37% |
| long (400 B) | 287 µs | **213 µs** | -28% |

### 内訳 (どこに時間が溶けているか)

`benches/lindera_share.rs` で形態素解析だけを切り出して比較した結果
(同程度の byte 数、 最適化前):

| 入力 | `to_ruby()` | Lindera 単体 | 差分 (辞書 lookup + path 選択) |
|---|---|---|---|
| bucket が巨大な字 主体 (84 B) | 128.5 µs | 14.2 µs | 114 µs |
| bucket 1 件の字 主体 (78 B) | 36.8 µs | 10.3 µs | 26.5 µs |

**同じ長さの日本語なのに 3.5 倍差**があり、 Lindera は全体の 11〜28% にすぎない。
支配項は形態素解析ではなく **辞書 bucket の走査**だった (→ §3 で解消)。

### allocation churn

415 B の `to_ruby` 1 回で **2,947 allocs / 415 KiB** (≒ 7.1 alloc/byte、 入力長にほぼ線形)。
candidate 収集バッファを使い回す前は 3,089 allocs / 490 KiB だった (= 位置ごとの
`Vec` 確保と伸長時の再確保)。

残りの主因は投機的に生成する `Candidate` の `surface` / `reading` の `String` clone。
`surface` は常に `input[range]` と同一で本来不要だが、 `AnalyzeResult.candidates` が
0.1.0 で freeze された public field のため、 `Candidate` から field を落とすことも
lifetime を付けて借用化することも **破壊的変更**になる。 手を付けるなら
`candidates` の型を見直す節目 (0.5.0 等) で。

### 計測上の注意

この開発機の criterion ノイズ底は **±5〜8%** (コード無変更の再実行でも
「-5.5% improved」 と 「+8.2% regressed」 が両方出る)。 A/B 判断の前に
無変更で 2 回回してノイズ底を取ること。 重い bench を 2 本並列で走らせると
メモリ不足で kill される。

## 3. 最適化の履歴

- **0.1.5 (2026-06-10): dict lookup hot path を約 200x 高速化**
  実 dict (当時 47k entry) で `to_ruby` の long 文 latency **38 ms → 0.19 ms**、
  短文 1.8 ms → 8 µs。 `DictBridgeProvider` が各 byte 位置で全 entry を
  linear prefix scan (O(N×M)) していたのを、 surface 先頭 char で逆引きする
  lazy index (`rich_index` / `kanji_index`、 `OnceLock`) で O(N×k) 化。
  corpus 回帰 801 件 / lib test 429 件 pass で挙動同一を確認済。
- **0.1.0 stable cut 時点**: dict-curated context rule 路線 ([[kanji]] block +
  next_starts match) に統一しても path scoring overhead は発生せず、
  alpha.13 baseline と同等 latency を維持。

- **2026-09-17: 先頭 2 文字での区間絞り込みで実文 1.4〜1.8 倍速**
  0.1.5 の先頭 char 逆引き index は 「bucket を引いてから **全件走査**」 だった。
  bucket 分布は p50=1 / p90=1 / p99=14 と大半が小さい一方、 **実文で頻出する字に
  集中** していた (御 713 / 大 358 / 三 187 / 小 184 / 天 172 / 一 163 / 白 136 /
  中 121 / 水 118 / 何 111)。 = 「御」 が 1 文字出るたび 713 回の前方一致判定。
  bucket は surface 昇順 sort 済み (列挙順の決定性のため) なので、 先頭 2 文字が
  一致する連続区間を `partition_point` 2 回で取る方式に変更
  (`Dict::rich_matching_prefix`)。 列挙順・候補集合ともに不変、
  corpus 11,057 件 / lib test 600 件 pass。

辞書の entry 数そのものより、 **先頭文字 bucket の偏り**が効く構造だった。
しかもこの偏りは辞書改善の vein (御X prefix / 数詞+助数詞 / 姓・地名 suffix) が
そのまま育てるので、 **辞書を良くするほど遅くなる**関係にあった。
2 文字絞り込み後は実走査が 「その 2 文字で始まる entry 数」 に固定されるため、
今後 bucket が太っても per-call コストは増えない。

## 4. リソース

| 項目 | 値 |
|---|---|
| binary サイズ | 67.6 MB (IPADIC 同梱) |
| 辞書ファイル | 170 ファイル / 3.2 MB (core 3.1 MB + rules 113 KB) |
| entry 数 | 66,382 (jukugo 22,936 + unihan 43,446) |
| 回帰コーパス | 11,057 件 (全 pass) |
| peak メモリ | **108 MB** (50,000 行 / 1.61 MB 処理時の peak WorkingSet) |

## 5. 再現手順

```bash
cd furigana-dict
head -n 50000 <コーパス>.tsv > /tmp/bench50k.txt
time ../furigana/target/release/furigana.exe \
  lookup --mode hiragana --core-dict-dir core --rules-dir rules --batch \
  < /tmp/bench50k.txt > /dev/null
```

起動コストを差し引くには、 1 行だけの入力で同じコマンドを実行して切片を測る。

peak メモリは Windows では `/usr/bin/time -f %M` が無いので PowerShell で:

```powershell
$exe = Join-Path $PWD "../furigana/target/release/furigana.exe"
$p = Start-Process -FilePath $exe `
  -ArgumentList "lookup --mode hiragana --core-dict-dir core --rules-dir rules --batch" `
  -RedirectStandardInput bench50k.txt -RedirectStandardOutput out.txt -PassThru -NoNewWindow
$peak=0; while(-not $p.HasExited){ $p.Refresh(); if($p.WorkingSet64 -gt $peak){$peak=$p.WorkingSet64}; Start-Sleep -Milliseconds 50 }
[math]::Round($peak/1MB,1)
```

(stdin へパイプで流し込むと 6 MB 級で詰まるため、 `-RedirectStandardInput` で
ファイル指定すること)

## 6. 未計測 / TODO

- ~~criterion bench の再実行 (現行 0.4.3 / 66k entry での更新値)~~ → 2026-09-17 実施 (§2)。
- `Candidate` の `String` clone 削減 (alloc 7.4 回/byte、 §2 参照)。 provider が
  `&mut Vec` へ push する形に変えて候補バッファを再利用する案。
- HTTP serve 経由の p50/p99 latency (常駐時のスループット)。
