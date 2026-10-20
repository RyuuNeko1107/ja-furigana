//! `furigana serve` サブコマンド
//!
//! ローカル HTTP サーバー。default bind は `127.0.0.1:8000`。
//! API は 公開 Furigana API と互換 (drop-in 差し替え可能)。
//!
//! ## 構成
//! - [`types`]    : リクエスト / レスポンス型 + AppState
//! - [`handlers`] : `/furigana` `/healthz` のハンドラ + 変換ロジック
//! - [`auth`]     : 認証ミドルウェア + CORS
//!
//! ## エンドポイント
//! - `GET  /furigana?text=...&mode=tts|hiragana|ruby|kanji`
//! - `POST /furigana`  body: `{ "text": "...", "mode": "...", ... }`
//! - `GET  /healthz`   ヘルスチェック
//!
//! ## 認証
//! `X-API-Key: <token>` (公開 API 互換、優先) または
//! `Authorization: Bearer <token>` (fallback)。
//! `config.toml` の `[auth].tokens` または起動時 `--token`
//! (env `FURIGANA_TOKEN`) が空でない場合のみ必須化。
//! `/healthz` は常に認証不要。

mod auth;
mod handlers;
mod metrics;
mod types;

use crate::config::Config;
use crate::paths::Paths;
use anyhow::{anyhow, Result};
use axum::{
    middleware,
    routing::{get, post},
    Router,
};
use clap::Args as ClapArgs;
use furigana::Furigana;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::RwLock;

use auth::{build_cors, require_admin_token, require_token};
#[cfg(unix)]
use handlers::do_reload;
use handlers::{
    admin_reload, furigana_get, furigana_post, healthz, metrics as metrics_handler, ReloadSource,
};
use metrics::ServerMetrics;
use types::AppState;

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// bind address (`config.toml [server].bind` を上書き)
    #[arg(long)]
    pub bind: Option<String>,

    /// 認証トークン (env `FURIGANA_TOKEN`)。
    /// 指定すると `/furigana` で `X-API-Key` または Bearer 必須化。
    #[arg(long, env = "FURIGANA_TOKEN")]
    pub token: Option<String>,

    /// 起動時に GitHub Releases から最新の `ja-furigana-dict` を自動取得してから
    /// listen を開始する。trip 失敗時は warn を出して既存辞書で起動を続行
    /// (network なし環境でも壊れない)。`config.toml [auto_update].pin` で版を固定可。
    #[arg(long)]
    pub auto_pull: bool,

    /// rule-based accent 推定を有効化する (ADR-0007)。 mode=accent / voicevox-aques / aquestalk で
    /// dict bracket が無い token にも外来語 -3 rule / 人名 rule の推定 accent が付く
    /// (推定 phrase は `"estimated": true` で真値と区別)。 読み出力には影響しない。
    #[arg(long)]
    pub estimate_accent: bool,

    /// dict のアクセント専用の表 (role = "accent") を読まない (読み込み時間と常駐メモリの節約。
    /// 読み出力には影響しない)
    #[arg(long)]
    pub no_accent_lexicon: bool,

    /// dict のこの相対 path (dir なら配下全部、 file なら 1 file) を読まない。 複数回指定可。
    /// reload 後も維持する
    #[arg(long = "exclude-dict", value_name = "PATH")]
    pub exclude_dict: Vec<String>,
}

pub fn run(args: Args, paths: &Paths, cfg: &Config) -> Result<()> {
    // ─── 起動時 auto-pull (admin token 不要、公開 GitHub Release から取得) ────
    if args.auto_pull {
        let pin = if cfg.auto_update.pin.is_empty() {
            None
        } else {
            Some(cfg.auto_update.pin.as_str())
        };
        tracing::info!(
            "--auto-pull: {} を取得します",
            pin.unwrap_or("最新 release")
        );
        if let Err(e) = super::dict_pull::run(paths, pin) {
            tracing::warn!("起動時 dict pull に失敗 ({e})。既存辞書で起動を続行します。");
        }
    }

    let furigana_inner =
        super::apply_dict_excludes(super::furigana_builder(paths), &args.exclude_dict)
            .estimate_accent(args.estimate_accent)
            .accent_lexicon(!args.no_accent_lexicon)
            .build()?;
    // server は最初のリクエストレイテンシを下げるため、Lindera analyzer を eager init。
    // build_furigana 自体は lazy なので listen 前にここで明示的に init して
    // 起動失敗を listen 前に検知できるようにもする。
    furigana_inner
        .preload()
        .map_err(|e| anyhow!("形態素解析器の初期化に失敗: {e}"))?;
    // 数字・URL・絵文字などの正規表現のコンパイルや各経路の初回の確保も先に済ませる
    // (最初に使われた機能のリクエストだけ数 ms 遅くなるのを防ぐ)
    furigana_inner.warm_up();
    let furigana: Arc<Furigana> = Arc::new(furigana_inner);

    let bind = args.bind.unwrap_or_else(|| cfg.server.bind.clone());
    let mut tokens = cfg.auth.tokens.clone();
    if let Some(t) = args.token {
        tokens.push(t);
    }
    let auth_enabled = !tokens.is_empty();
    let admin_tokens = cfg.auth.admin_tokens.clone();
    let admin_enabled = !admin_tokens.is_empty();

    let server_metrics = Arc::new(ServerMetrics::default());
    server_metrics.set_dict_size(furigana.dict_size());

    let state = AppState {
        furigana: Arc::new(RwLock::new(furigana)),
        tokens: Arc::new(tokens),
        admin_tokens: Arc::new(admin_tokens),
        paths: Arc::new(paths.clone()),
        metrics: server_metrics,
        estimate_accent: args.estimate_accent,
        accent_lexicon: !args.no_accent_lexicon,
        dict_excludes: Arc::new(args.exclude_dict.clone()),
    };

    let cors = build_cors(cfg);

    // ─── セキュリティ: HTTP body 上限 + rate limit ───
    // body 1 MB は通常のリクエスト (text パラメータ ~100k chars) より十分大きい。
    // rate limit 60 req/min per IP (= 1 req/sec バースト 5)。 admin endpoint も
    // 含めてグローバルに適用、 過剰トラフィックは即 429 で弾く。
    use axum::body::Body;
    use axum::http::Request;
    use tower_governor::governor::GovernorConfigBuilder;
    use tower_governor::GovernorLayer;
    use tower_http::limit::RequestBodyLimitLayer;
    use tower_http::timeout::TimeoutLayer;
    use tower_http::trace::TraceLayer;
    use tracing::Level;
    const MAX_BODY_BYTES: usize = 1024 * 1024;
    // 1 リクエストの上限処理時間。 変換自体は通常 sub-ms〜数 ms なので、
    // これを超えるのは異常 (ハング / 想定外の重い入力)。 408 で打ち切り、
    // ワーカーを 1 リクエストに占有され続けないようにする defense-in-depth。
    const REQUEST_TIMEOUT_SECS: u64 = 30;
    // key は既定で接続元 IP。 リバースプロキシ配下では全員が同じ IP になるので、
    // `trust_proxy_headers = true` で X-Forwarded-For / X-Real-IP / Forwarded を使う。
    let governor_conf = std::sync::Arc::new(
        GovernorConfigBuilder::default()
            .per_second(cfg.rate_limit.per_second)
            .burst_size(cfg.rate_limit.burst_size)
            .finish()
            .ok_or_else(|| anyhow!("governor config 構築失敗"))?,
    );
    let governor_layer = GovernorLayer {
        config: governor_conf,
    };
    let proxy_governor_layer = if cfg.rate_limit.trust_proxy_headers {
        use tower_governor::key_extractor::SmartIpKeyExtractor;
        Some(GovernorLayer {
            config: std::sync::Arc::new(
                GovernorConfigBuilder::default()
                    .key_extractor(SmartIpKeyExtractor)
                    .per_second(cfg.rate_limit.per_second)
                    .burst_size(cfg.rate_limit.burst_size)
                    .finish()
                    .ok_or_else(|| anyhow!("governor config 構築失敗"))?,
            ),
        })
    } else {
        None
    };

    let furigana_routes = Router::new()
        .route("/furigana", get(furigana_get).post(furigana_post))
        .layer(middleware::from_fn_with_state(state.clone(), require_token));

    let admin_routes = Router::new()
        .route("/admin/reload", post(admin_reload))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_token,
        ));

    // TraceLayer: /healthz と /metrics は polling 系で log を埋めるので TRACE 降格、
    // それ以外 (= /furigana / /admin) は DEBUG で出す。 RUST_LOG=info でも
    // TraceLayer 自体は黙る (= 詳細は handler の debug! 経由で見る)。
    let trace_layer = TraceLayer::new_for_http().make_span_with(|req: &Request<Body>| {
        let path = req.uri().path();
        let level = if path == "/healthz" || path == "/metrics" {
            Level::TRACE
        } else {
            Level::DEBUG
        };
        tracing::span!(
            Level::DEBUG,
            "request",
            method = %req.method(),
            uri = %req.uri(),
            version = ?req.version(),
            trace_level = ?level,
        )
    });

    // 429 detection: governor_layer の 後段 (= 外側 wrap) で response status を見て
    // rate_limited counter を増やす + warn log を出す。 layer 順序は逆順 apply
    // なので、 .layer() chain で governor_layer より「後に」 書くと先に発火する。
    let log_429_state = state.clone();
    let log_429 = axum::middleware::from_fn(
        move |req: axum::extract::Request, next: axum::middleware::Next| {
            let metrics = log_429_state.metrics.clone();
            async move {
                let peer = req
                    .extensions()
                    .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                    .map(|c| c.0.to_string())
                    .unwrap_or_else(|| "?".to_string());
                let path = req.uri().path().to_string();
                let resp = next.run(req).await;
                if resp.status() == axum::http::StatusCode::TOO_MANY_REQUESTS {
                    metrics.record_rate_limited();
                    tracing::warn!(peer = %peer, path = %path, "rate limited (429)");
                }
                resp
            }
        },
    );

    let base = Router::new()
        .merge(furigana_routes)
        .merge(admin_routes)
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics_handler))
        .layer(cors)
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS),
        ))
        .layer(log_429);
    let app = match proxy_governor_layer {
        Some(layer) => base.layer(layer).layer(trace_layer),
        None => base.layer(governor_layer).layer(trace_layer),
    }
    .with_state(state.clone());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async move {
        let listener = TcpListener::bind(&bind)
            .await
            .map_err(|e| anyhow!("bind {bind} に失敗: {e}"))?;
        let actual = listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| bind.clone());

        tracing::info!("furigana serving on http://{actual}");
        if cfg.server.cors_origins.is_empty() {
            tracing::info!("CORS: 全オリジン許可 ([server].cors_origins が空)");
        }
        tracing::info!(
            "レート制限の key: {}",
            if cfg.rate_limit.trust_proxy_headers {
                "X-Forwarded-For / X-Real-IP / Forwarded (無ければ接続元 IP)"
            } else {
                "接続元 IP"
            }
        );
        if auth_enabled {
            tracing::info!("認証 (/furigana): 有効 (X-API-Key または Bearer)");
        } else {
            tracing::info!("認証 (/furigana): 無効 (ローカル想定)");
        }
        if admin_enabled {
            tracing::info!("admin (/admin/reload): 有効 (admin_tokens で認証)");
        } else {
            tracing::info!("admin (/admin/reload): 無効 ([auth].admin_tokens を設定すると有効化)");
        }

        // Unix: SIGHUP で reload (本番運用で `kill -HUP <pid>` で辞書再読込できるように)
        #[cfg(unix)]
        spawn_sighup_reload(state.clone());

        // 定期 auto-update: 公開 GitHub Releases を polling して新 tag があれば自動 reload
        if cfg.auto_update.enabled {
            spawn_auto_update(state.clone(), cfg.auto_update.clone());
        } else {
            tracing::info!(
                "auto_update: 無効 ([auto_update].enabled = true で起動時に polling task を生やす)"
            );
        }

        // tower_governor の PeerIpKeyExtractor が ConnectInfo<SocketAddr> を要求する
        // ため into_make_service_with_connect_info で peer IP extension を inject。
        // 未指定だと 500 「Unable To Extract Key!」 になる。
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| anyhow!("server error: {e}"))?;

        anyhow::Ok(())
    })?;

    tracing::info!("シャットダウン完了");
    Ok(())
}

/// `[auto_update]` の polling task を spawn する。
///
/// 動作:
/// 1. `interval` を待つ (起動直後の polling は省略、急いで叩かない設計)
/// 2. GitHub API で `ja-furigana-dict` の最新 tag を取得
///    (`pin` が空なら latest、空でなければそれを期待値として比較)
/// 3. 反映済 tag (memory 上に保持) と違えば `dict_pull` + 再 build + state swap
/// 4. ループ
///
/// admin_tokens 設定不要 (内部呼び出しで HTTP 経由しない)。失敗時は warn を出し
/// 既存辞書で稼働を継続。
fn spawn_auto_update(state: AppState, cfg: crate::config::AutoUpdateConfig) {
    let interval = auto_update_interval(&cfg.interval);
    tracing::info!(
        "auto_update: 有効 (interval={:?}, pin={})",
        interval,
        if cfg.pin.is_empty() {
            "(latest)"
        } else {
            &cfg.pin
        }
    );

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // 初回 tick は即時消費して、次の interval 経過後に最初の polling
        ticker.tick().await;

        // memory 上に「最後に反映した tag」を保持。process 再起動で初期化される
        // (pin 指定時はそのまま、未指定時は latest を 1 回引いてから比較開始)
        let mut last_applied: Option<String> = if cfg.pin.is_empty() {
            None
        } else {
            Some(cfg.pin.clone())
        };
        // 中身が使えないと判った tag (build できない・件数が少なすぎる)。 同じ tag は何度取得しても
        // 同じ結果になるので、 別の tag が出るまで再取得しない (毎回 数十 MB を取り直さない)
        let mut rejected: Option<String> = None;

        loop {
            ticker.tick().await;

            let target_tag = if cfg.pin.is_empty() {
                match crate::commands::dict_pull::resolve_latest_tag_async().await {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::warn!("auto_update: latest tag 取得失敗: {e}");
                        continue;
                    }
                }
            } else {
                cfg.pin.clone()
            };

            if let Some(why) =
                skip_reason(&target_tag, last_applied.as_deref(), rejected.as_deref())
            {
                tracing::debug!("auto_update: {} を skip ({why})", target_tag);
                continue;
            }

            tracing::info!("auto_update: {} を取得 + reload します", target_tag);

            let paths = state.paths.clone();
            let target_owned = target_tag.clone();
            let result = tokio::task::spawn_blocking(move || {
                crate::commands::dict_pull::run(&paths, Some(&target_owned))
            })
            .await;

            match result {
                Ok(Ok(())) => {
                    // pull 成功 → state を swap
                    match handlers::do_reload(&state, ReloadSource::AutoUpdate).await {
                        Ok(size) => {
                            tracing::info!(
                                "auto_update: {} 反映完了 (dict_size={})",
                                target_tag,
                                size
                            );
                            last_applied = Some(target_tag);
                        }
                        Err(e) => {
                            tracing::warn!("auto_update: reload 失敗: {e}");
                        }
                    }
                }
                Ok(Err(e)) => {
                    if e.downcast_ref::<crate::commands::dict_pull::DictRejected>()
                        .is_some()
                    {
                        // 稼働中の辞書はそのまま。 この tag は次の tag が出るまで試さない
                        tracing::error!("auto_update: {target_tag} は使えない辞書でした: {e:#}");
                        rejected = Some(target_tag);
                    } else {
                        tracing::warn!("auto_update: dict pull 失敗: {e}");
                    }
                }
                Err(e) => {
                    tracing::warn!("auto_update: spawn_blocking join error: {e}");
                }
            }
        }
    });
}

/// 自動更新でこの tag を取得しない理由 (取得するなら `None`)。
fn skip_reason(
    target: &str,
    last_applied: Option<&str>,
    rejected: Option<&str>,
) -> Option<&'static str> {
    if last_applied == Some(target) {
        Some("反映済み")
    } else if rejected == Some(target) {
        Some("使えない辞書と判定済み")
    } else {
        None
    }
}

/// シンプルな期間 parser: `"30m" / "1h" / "6h" / "1d" / "3600"` 等を受ける
fn parse_duration(s: &str) -> Option<std::time::Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (num_part, mult) = if let Some(n) = s.strip_suffix('h') {
        (n, 3600)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60)
    } else if let Some(n) = s.strip_suffix('d') {
        (n, 86400)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1)
    } else {
        (s, 1) // 純粋な数字なら秒
    };
    // 掛け算が溢れる値 (release では黙って巻き戻る) は解釈できない扱いにする
    num_part
        .trim()
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(mult))
        .map(std::time::Duration::from_secs)
}

/// `auto_update.interval` の下限。 0 秒は `tokio::time::interval` が panic して更新が止まり、
/// 極端に短いと GitHub API (未認証は 60 回/時) を使い切って失敗を繰り返す。
const MIN_AUTO_UPDATE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(300);

/// 設定文字列から polling 間隔を決める。 解釈できなければ 24h、 下限未満なら下限に丸める。
fn auto_update_interval(raw: &str) -> std::time::Duration {
    match parse_duration(raw) {
        None => {
            tracing::warn!(
                "auto_update.interval='{raw}' を解釈できませんでした (例: 1h / 30m / 1d)。 24h にフォールバック。"
            );
            std::time::Duration::from_secs(24 * 3600)
        }
        Some(d) if d < MIN_AUTO_UPDATE_INTERVAL => {
            tracing::warn!(
                "auto_update.interval='{raw}' は短すぎます。 下限の {:?} に丸めます。",
                MIN_AUTO_UPDATE_INTERVAL
            );
            MIN_AUTO_UPDATE_INTERVAL
        }
        Some(d) => d,
    }
}

/// SIGHUP を listen して `do_reload` を呼ぶバックグラウンドタスクを起動。
/// 本番運用 (systemd 等) で `systemctl reload furigana` 相当の挙動。
/// Windows ではビルドから除外される。
#[cfg(unix)]
fn spawn_sighup_reload(state: AppState) {
    use tokio::signal::unix::{signal, SignalKind};
    tokio::spawn(async move {
        let mut sighup = match signal(SignalKind::hangup()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("SIGHUP handler 初期化失敗: {e}");
                return;
            }
        };
        while sighup.recv().await.is_some() {
            tracing::info!("SIGHUP を受信、辞書を reload します");
            match do_reload(&state, ReloadSource::Sighup).await {
                Ok(size) => tracing::info!("reload 成功 (dict_size={size})"),
                Err(e) => tracing::error!("reload 失敗: {e}"),
            }
        }
    });
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => tracing::info!("SIGINT を受信、シャットダウンします"),
            _ = sigterm.recv() => tracing::info!("SIGTERM を受信、シャットダウンします"),
        }
    }
    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
        tracing::info!("Ctrl+C を受信、シャットダウンします");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn parse_duration_units_and_invalid() {
        assert_eq!(parse_duration("30m"), Some(Duration::from_secs(1800)));
        assert_eq!(parse_duration("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_duration("1d"), Some(Duration::from_secs(86400)));
        assert_eq!(parse_duration("3600s"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_duration("3600"), Some(Duration::from_secs(3600))); // 単位なし = 秒
                                                                             // 不正 → None (= caller は default にフォールバック)
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration("1.5h"), None); // 非整数
    }

    #[test]
    fn parse_duration_rejects_overflow() {
        // 掛け算が u64 を溢れる値は、 巻き戻らずに None
        assert_eq!(parse_duration("213503982334602d"), None);
        assert_eq!(parse_duration("18446744073709551615h"), None);
        assert_eq!(parse_duration("18446744073709551615m"), None);
        // 溢れない最大 (単位なし = 秒) はそのまま
        assert_eq!(
            parse_duration("18446744073709551615"),
            Some(Duration::from_secs(u64::MAX))
        );
    }

    #[test]
    fn auto_update_interval_has_floor_and_default() {
        // 0 / 極端に短い → 下限 (0 のままだと tokio::time::interval が panic する)
        assert_eq!(auto_update_interval("0"), MIN_AUTO_UPDATE_INTERVAL);
        assert_eq!(auto_update_interval("1s"), MIN_AUTO_UPDATE_INTERVAL);
        assert_eq!(auto_update_interval("299"), MIN_AUTO_UPDATE_INTERVAL);
        // 下限ちょうど以上はそのまま
        assert_eq!(auto_update_interval("5m"), Duration::from_secs(300));
        assert_eq!(auto_update_interval("6h"), Duration::from_secs(6 * 3600));
        // 解釈できない / 溢れる → 24h
        assert_eq!(auto_update_interval("abc"), Duration::from_secs(86400));
        assert_eq!(
            auto_update_interval("213503982334602d"),
            Duration::from_secs(86400)
        );
    }

    #[test]
    fn auto_update_skips_applied_and_rejected_tags_but_not_new_ones() {
        // 反映済みは取らない
        assert_eq!(skip_reason("v1", Some("v1"), None), Some("反映済み"));
        // 使えないと判った tag は、 次の tag が出るまで取り直さない
        assert_eq!(
            skip_reason("v2", Some("v1"), Some("v2")),
            Some("使えない辞書と判定済み")
        );
        // 別の tag が出れば取る (却下は前の tag のぶんだけ)
        assert_eq!(skip_reason("v3", Some("v1"), Some("v2")), None);
        // 初回 (何も反映していない) は取る
        assert_eq!(skip_reason("v1", None, None), None);
    }
}
