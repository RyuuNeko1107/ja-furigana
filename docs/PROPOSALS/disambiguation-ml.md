# Proposal: 同形異音語 ML 曖昧解決

**Status**: Draft (2026-05-25)
**Location**: `プロジェクト/ml/` (既存内容は破棄、新規構築)
**依存**: ADR-0004 (ambiguous reading candidates)、furigana lib `--mode=accent` JSON

## 1. 目的

ja-furigana lib が `ambiguous: true` で返す同形異音語候補を、文脈に基づいて正しく選択する ML モデルを開発する。

例: 「相手より上手だった」 → lib は {ジョウズ, カミテ, ウワテ} を返す → ML が「ウワテ」を選択。

## 2. 段階的アプローチ

### Phase 1: LLM で教師データ作成

1. **候補収集**: dict の `[[alt]]` entry から同形異音語リストを抽出
2. **文脈生成**: 各候補の典型的な使用文を LLM に生成させる + 実コーパス (stream-comments SQLite / signal_log) から自然文を収集
3. **ラベリング**: LLM に文脈 + 候補リストを渡して正解 reading を選択させる
4. **検証**: NHK アクセント辞典 / 大辞林の用例と照合、明らかな誤りを修正
5. **出力**: `{context: str, surface: str, candidates: [str], label: str}` の JSONL

**目標データ量**: 主要同形異音語 50-100 語 × 各 50-200 文脈 = 5K-20K sample

### Phase 2: 軽量 classifier 蒸留

1. **入力**: surface + 前後 N token (N=3-5) の文字列
2. **モデル**: char-level Transformer (小) or LSTM、candidate embedding + context encoding → softmax over candidates
3. **学習**: Phase 1 の教師データで fine-tune
4. **frozen 化**: ONNX export → deterministic 推論 (同 input → 同 output)
5. **評価**: held-out set で accuracy、VV 比較との cross-check

**制約**:
- モデルサイズ: < 50MB (API server に載る)
- 推論速度: < 10ms/token (リアルタイム TTS 前段)
- deterministic: frozen weights、同一 input → 同一 output

### Phase 3: production 統合

1. **下流の API wrapper**: `ambiguous: true` token を検出 → ML model に投げる → reading 上書き
2. **fallback**: model load 失敗 / タイムアウト時は lib の weight ベース top pick を使用
3. **signal_log**: ML 選択結果を記録、定期的に精度評価

## 3. データパイプライン

```
dict [[alt]] entries
        │
        ▼
同形異音語リスト (surface → candidates)
        │
        ├── LLM 文脈生成 (synthetic)
        ├── stream-comments SQLite (natural)
        └── signal_log (production)
        │
        ▼
文脈 + surface + candidates + label (JSONL)
        │
        ▼
train / eval / test split (80/10/10)
        │
        ▼
lightweight classifier (char Transformer)
        │
        ▼
ONNX frozen model
        │
        ▼
downstream API wrapper integration
```

## 4. ディレクトリ構成 (プロジェクト/ml/)

```
ml/
├── README.md
├── data/
│   ├── candidates.json      # dict から抽出した同形異音語リスト
│   ├── labeled/              # LLM ラベル済み JSONL
│   └── corpus/               # 自然文コーパス
├── scripts/
│   ├── extract_ambiguous.py  # dict → candidates.json
│   ├── generate_contexts.py  # LLM で文脈生成
│   ├── label_with_llm.py     # LLM でラベリング
│   ├── train.py              # classifier 学習
│   ├── eval.py               # 評価
│   └── export_onnx.py        # ONNX export
├── models/
│   └── disambiguation/       # frozen model artifacts
└── requirements.txt
```

## 5. 主要同形異音語 (初期ターゲット)

| surface | candidates | 頻度 |
|---|---|---|
| 上手 | ジョウズ / カミテ / ウワテ | 高 |
| 下手 | ヘタ / シモテ | 高 |
| 生 | セイ / ショウ / ナマ / イ / ウ / キ / ハ | 高 |
| 日 | ニチ / ヒ / ジツ / カ | 高 |
| 人気 | ニンキ / ヒトケ | 中 |
| 大人 | オトナ / タイジン | 中 |
| 一人 | ヒトリ / イチニン | 中 |
| 今日 | キョウ / コンニチ | 中 |
| 明日 | アシタ / アス / ミョウニチ | 中 |
| 昨日 | キノウ / サクジツ | 中 |
| 風 | カゼ / フウ | 中 |
| 物 | モノ / ブツ / モツ | 中 |
| 行 | ギョウ / コウ / イ / ユ / オコナ | 高 |
| 間 | アイダ / マ / カン / ケン | 高 |
| 方 | カタ / ホウ | 高 |

## 6. 評価指標

- **accuracy**: held-out set での正解率 (target: > 90%)
- **VV match**: VOICEVOX engine 出力との一致率改善幅
- **latency**: 1 token あたりの推論時間 (target: < 10ms)
- **coverage**: ambiguous token のうち model が判定できた割合

## 7. リスク / 未解決

- LLM ラベリングの精度: 日本語の微妙なニュアンスで LLM が間違える可能性。人手レビュー必要。
- 訓練データの偏り: synthetic data に偏ると自然文での精度が落ちる。stream-comments で補う。
- 単漢字 (生/日/行) の候補数が多い: 文脈窓を広げるか、品詞ヒントが必要かも。
- lib の deterministic 制約との整合: model は下流の API wrapper 側で使う、lib には入れない。
