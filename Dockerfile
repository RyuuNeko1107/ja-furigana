# syntax=docker/dockerfile:1.6
#
# Multi-stage build for furigana-cli
#
# Build stage: rust:1.89-slim-bookworm (matches Cargo.toml MSRV、std::fs::File::lock が 1.89 で安定化)
#   ★runtime と同じ Debian 12 にすること。 `rust:1.89-slim` は Debian 13 (glibc 2.41) で、
#   distroless/cc-debian12 (glibc 2.36) では `GLIBC_2.38 not found` で起動しない。
# Runtime stage: distroless/cc-debian12:nonroot (small, no shell, libgcc あり、非 root で実行)
#
# ベース画像は digest で固定している (タグが指す中身が差し替わっても同じ画像で build する)。
# 更新は Dependabot (docker) が PR にする。

FROM rust:1.99-slim-bookworm@sha256:a24e292b4af7bdee351a31d3a6379972a5652f0ea22a1b6d6883324ac4f5097c AS builder

WORKDIR /build

# 依存だけ先に build してキャッシュを効かせる手もあるが、
# pre-alpha では単純に全 build。最適化は後で。
COPY . .

# 並列数を絞りたい環境 (メモリの少ない CI 等) 用: --build-arg BUILD_JOBS=2
ARG BUILD_JOBS=

# release build (lto + codegen-units=1 で size 最適化済み、Cargo.toml 設定参照)
# --locked: Cargo.lock どおりの依存で build する (無ければ失敗)。 配布する bin だけを build する
RUN cargo build --release --locked -p ja-furigana-cli --bin furigana ${BUILD_JOBS:+--jobs "$BUILD_JOBS"}

# 非 root の実行ユーザー (distroless nonroot = 65532) が書ける /data を用意する。
# volume は初回に、 イメージ内のこのディレクトリの所有者を引き継ぐ。
RUN mkdir /data-skel

# ─── runtime ───────────────────────────────────────────────────────────────
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f

LABEL org.opencontainers.image.source="https://github.com/RyuuNeko1107/furigana"
LABEL org.opencontainers.image.description="日本語フリガナ (ルビ) 解決 HTTP サーバー"
LABEL org.opencontainers.image.licenses="MIT"

# ビルド成果物の配置 (data/rules/* は include_str! で binary に embed 済)
COPY --from=builder /build/target/release/furigana /usr/local/bin/furigana
COPY --from=builder --chown=65532:65532 /data-skel /data

# 非 root (65532) で動かす。 bind mount で /data を渡す場合は、 ホスト側のディレクトリを
# このユーザーが書けるようにしておく (例: chown 65532:65532)。
USER 65532:65532

# 既定の bind: 0.0.0.0:8000 (コンテナ内なので外部公開前提)
EXPOSE 8000

# user dict / config を mount するなら /data 推奨 (FURIGANA_DATA_DIR で指定)
ENV FURIGANA_DATA_DIR=/data
VOLUME ["/data"]

ENTRYPOINT ["/usr/local/bin/furigana"]
CMD ["serve", "--bind", "0.0.0.0:8000"]
