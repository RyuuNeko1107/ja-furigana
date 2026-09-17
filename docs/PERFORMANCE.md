# エンジン性能 (furigana 0.4.3)

## 0. 性能目標 (2026-09-18 決定)

精度を優先するが、 読み上げ BOT / 公開 API の hot path なので **処理量の下限** を決め、
精度改善はその予算の範囲で行う。

| 種別 | 基準 |
|---|---|
| **下限** (割ったら不可) | 実コメント行 (平均 30 B 前後) を 1 スレッドで **10,000 行/秒 以上** |
| 長文 | 1,000 文字の入力 1 回を **5 ms 以内** |
| スケーリング | 入力長に対して線形 (位置ごとに入力全体・候補全体を走査する O(N²) を入れない) |
| **1 変更あたり** | 実コーパス A/B で処理時間 **+10% 以内**。 超える場合は精度の利得と並べて判断する |

2026-09-18 時点の実測は約 57,000 行/秒 (下限の約 5.7 倍)。 この差が精度改善に使える予算。

### 計測方法 (A/B)

criterion はこの開発機でノイズ底が ±5〜8% あり (§2)、 数 % の変化を判定できない。
lib 変更の判定は次の方法に固定する:

1. 変更前の git ref と作業ツリーで、 それぞれ一括読み変換ツールをビルドする
2. 実コメント 30 万行 (漢字を含む行を seed 固定で無作為抽出) を **両版交互に 3 回** 流す
3. 各版の **最小時間** で比較する (バックグラウンド負荷の影響を受けにくい)
4. 出力差分を全件出し、 改善 / 退行を目視で分類する

メンテナ環境ではこれを 1 コマンドにしている (`stream-comments/scripts/lib_ab.py run --base <ref>`、
非公開ツール)。 確保量の比較が要る時は counting allocator 付きの一時 example で
alloc 回数 / bytes と出力 hash を併記する (§2)。

### 判定の実例

| 変更 | 時間 | 採否 |
|---|---|---|
| Lindera n-best の次点分割を edge に追加 | **約 5 倍** | 不採用 (精度も改善と退行が拮抗) |
| 同字連続の文脈判定 + 1 字一段動詞語幹 | +0.2% | 採用 |
| 確保削減 (Lattice 使い回し / 内部候補型) | -15〜20% | 採用 |

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

投機的に生成する `Candidate` の `surface` / `reading` の `String` clone は、
2026-09-18 に **公開 API を変えずに** 解消した: provider → Viterbi 間を内部型
`RawCandidate` (surface なし、 reading は借用) にし、 公開型 `Candidate` へは
採択 path と `analyze()` の `candidates` だけ変換する (provider trait は crate 内部なので非破壊)。
あわせて Lindera の `Lattice` を使い回し (短文 1 回の確保の最大項だった)、
pipeline 内部の形態素解析は品詞・活用の `String` を作らない軽量版にした。
実コメント行で **allocs -35〜43% / bytes -21〜39%**、 出力は完全に同一。

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

- **2026-09-18: 1 回あたりの確保を 35〜43% 削減** (§2 allocation churn)。
  Lindera fallback の位置 lookup も全 edge 走査から二分探索に変更 (長い入力での O(N²) 除去)。

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
- ~~`Candidate` の `String` clone 削減~~ → 2026-09-18 実施 (§2)。
- 残る確保の大物は Lindera 内部 (`Lattice::set_text` の一時配列、 `Token::details` の `Vec`) で upstream 側。
- HTTP serve 経由の p50/p99 latency (常駐時のスループット)。
