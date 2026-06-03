# 設定ファイル / 環境変数 / CLI フラグ

`furigana` CLI の設定ソース 3 経路と、それぞれの優先順位の解説。

> 戻る: [README](../README.md) / 関連: [HTTP_API.md](./HTTP_API.md) / [DATA_LAYOUT.md](./DATA_LAYOUT.md)

## 設定ファイル (`config.toml`)

`<data_dir>/config.toml` を読みます。default の `<data_dir>` は実行ファイルと同じディレクトリ ([DATA_LAYOUT.md](./DATA_LAYOUT.md) 参照)。`--config <path>` または `FURIGANA_CONFIG` 環境変数で別の場所を指定可能。

ファイルが存在しなければ default 値で起動 (全項目 optional):

```toml
[server]
bind = "127.0.0.1:8000"
cors_origins = []  # 空 = Any 許可 (ローカル用途) / 厳格化したいときは ["https://example.com"]

[auth]
tokens = []        # 空 = /furigana 認証無効 (ローカル想定)
admin_tokens = []  # 空 = /admin/* 機能 off (503)
```

### `[server]` セクション

| キー | 型 | default | 説明 |
|---|---|---|---|
| `bind` | string | `"127.0.0.1:8000"` | `furigana serve` の listen address |
| `cors_origins` | string[] | `[]` | CORS 許可オリジン。空なら **Any 許可** (ローカル用途で楽)。本番運用なら明示指定推奨 |

### `[auth]` セクション

| キー | 型 | default | 説明 |
|---|---|---|---|
| `tokens` | string[] | `[]` | `/furigana` 用トークン。空なら認証無効 |
| `admin_tokens` | string[] | `[]` | `/admin/reload` 用トークン。空なら admin 機能自体が 503 で off |

`tokens` と `admin_tokens` は **別系列**: 一般 `tokens` では `/admin/*` は通らない。これにより「読み取りはできるが reload はさせない」運用が可能。

> **`admin_tokens` は必須ではない**: 個人 / 単一プロセス運用なら設定不要。
> 外部 (別ホスト / 別 process) から HTTP 経由で reload を打ちたいときだけ必要。
> 辞書更新の他の経路は [HTTP_API.md#ホットリロード](./HTTP_API.md#ホットリロード) を参照。

### `[auto_update]` セクション (alpha.5+)

`furigana serve` 起動中の **定期 polling で自動 dict pull + reload** を行う仕組み。
**admin_tokens 不要** (内部呼び出しで HTTP 経由しないため)。

| キー | 型 | default | 説明 |
|---|---|---|---|
| `enabled` | bool | `false` | true で background polling task を spawn (opt-in) |
| `interval` | string | `"24h"` | polling 間隔。`"30m" / "1h" / "6h" / "1d" / "3600"` 等 |
| `pin` | string | `""` | ピン留めする tag (`"v2026.05.07"`)。空なら **最新追従** |

```toml
[auto_update]
enabled  = true
interval = "6h"
# pin = "v2026.05.07"   # コメントアウトで最新追従
```

GitHub API rate limit (60 req/h/IP) を考えて `interval` は **1h 以上推奨**。
失敗時は warn を出して既存辞書で稼働継続 (network なし環境でも壊れない)。

詳細な認証フローは [HTTP_API.md#認証](./HTTP_API.md#認証) / 辞書更新の全経路は
[HTTP_API.md#ホットリロード--自動更新](./HTTP_API.md#ホットリロード--自動更新) を参照。

### `[rate_limit]` セクション

`furigana serve` の per-IP レート制限 (`tower_governor`)。default は控えめなので、
信頼できる LAN 内や負荷試験では緩めると良い。

| キー | 型 | default | 説明 |
|---|---|---|---|
| `per_second` | int | `1` | 1 秒あたりに補充される許可リクエスト数 (per IP) |
| `burst_size` | int | `5` | バケットに貯められる上限 (= 瞬間バースト許容数) |

```toml
[rate_limit]
per_second = 20
burst_size = 100
```

> default (`1` / `5`) は素の公開エンドポイント保護向け。短文を高頻度で投げる
> 配信コメント用途や負荷試験では `per_second` を上げる (負荷試験は
> [`tools/k6_loadtest.js`](../tools/k6_loadtest.js) を参照)。

## 環境変数

`config.toml` より後で評価され、上書きする:

| 変数 | 役割 | 例 |
|---|---|---|
| `FURIGANA_DATA_DIR` | `<data_dir>` を上書き | `/var/lib/furigana` |
| `FURIGANA_CONFIG` | `config.toml` の path を上書き | `/etc/furigana/config.toml` |
| `FURIGANA_TOKEN` | `[auth].tokens` に 1 件追加 | `secret-token-xyz` |
| `RUST_LOG` | tracing log level | `info`, `debug`, `furigana=trace` |

`FURIGANA_TOKEN` は **既存 `tokens` を置き換えるのではなく追加** する。複数 token 運用にも干渉しない。

## CLI フラグ (最優先)

```sh
furigana --data-dir /var/lib/furigana \
         --config /etc/furigana/config.toml \
         --verbose \
         serve --bind 0.0.0.0:8000 --token secret
```

| フラグ | 役割 | 環境変数 fallback |
|---|---|---|
| `--data-dir <path>` | `<data_dir>` を上書き (グローバル) | `FURIGANA_DATA_DIR` |
| `--config <path>` | `config.toml` の path を上書き (グローバル) | `FURIGANA_CONFIG` |
| `-v, --verbose` | tracing log を info にする (グローバル) | (なし) |
| `serve --bind <addr>` | listen address を上書き | (なし) |
| `serve --token <t>` | 一般 token を 1 件追加 | `FURIGANA_TOKEN` |
| `serve --auto-pull` | 起動時に GitHub Releases から最新辞書を取得 (alpha.5+) | (なし) |

CLI flag → env → `config.toml` → default の優先順位で評価される。

## 設定例

### ローカル開発 (default のまま)

`config.toml` 不要、環境変数も不要。`furigana serve` で `127.0.0.1:8000` に listen、認証なし。

### 信頼できる LAN 内サーバー (token のみ)

```toml
# config.toml
[server]
bind = "0.0.0.0:8000"

[auth]
tokens = ["lan-shared-token-xxxxxx"]
```

```sh
furigana --data-dir /var/lib/furigana serve
```

### 公開 API + reload 経路を分離

```toml
# config.toml
[server]
bind = "0.0.0.0:8000"
cors_origins = ["https://your-frontend.example.com"]

[auth]
tokens = ["public-readonly-token"]
admin_tokens = ["ops-only-reload-token"]  # 別系列、reload 専用
```

```sh
# 通常の lookup
curl -H 'X-API-Key: public-readonly-token' \
  'https://api.example.com/furigana?text=灰桜&mode=ruby'

# 辞書 reload (admin 専用)
curl -X POST -H 'X-API-Key: ops-only-reload-token' \
  https://api.example.com/admin/reload
```

### Docker

```sh
docker run -d -p 8000:8000 \
  -v /host/data:/data \
  -e FURIGANA_DATA_DIR=/data \
  -e FURIGANA_TOKEN=secret \
  ghcr.io/ryuuneko1107/furigana:latest \
  furigana serve --bind 0.0.0.0:8000
```

`/host/data` を mount して、`furigana dict pull` の結果を host に永続化する想定。
