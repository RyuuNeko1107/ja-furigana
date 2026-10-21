# Security Policy

ja-furigana (lib `ja-furigana` + bin `ja-furigana-cli`) のセキュリティ報告窓口。

## English summary

- **Only the latest released version is supported** while the crates are `0.x` (currently `0.6.x`).
- **Report privately** via GitHub Security Advisories (`Security` → `Report a vulnerability`
  on https://github.com/RyuuNeko1107/ja-furigana). Email fallback: mail@ryuuneko.com.
- **Do not** open a public issue / PR / discussion thread for unpatched issues.
- Best-effort acknowledgement within 7 days. Hobby OSS project, single maintainer — SLAs not guaranteed.

## サポート対象

`0.x` の間、 セキュリティ修正は **常に最新のリリース** にのみ提供する (現在は `0.6.x`)。
古い版を使っている場合は、 最新版に上げる。 修正は `CHANGELOG.md` の `Security` 欄に書く。

| Version                     | Supported          |
| --------------------------- | ------------------ |
| latest release (`0.6.x`)    | :white_check_mark: |
| older releases              | :x:                |

`1.0` に到達した後は [SemVer](https://semver.org/lang/ja/) ポリシーに従い
直近 minor 系列に対してセキュリティ修正を提供する予定。

辞書 ([ja-furigana-dict](https://github.com/RyuuNeko1107/ja-furigana-dict)) は日次で自動リリースされ、
`furigana dict pull` は常に最新を取得する。

## 報告方法

ja-furigana に脆弱性を見つけた場合は、 **public な issue / PR / discussion を立てる前に**
以下のいずれかで非公開に連絡する:

1. **GitHub Security Advisories** (推奨):
   https://github.com/RyuuNeko1107/ja-furigana/security/advisories/new
2. **メール**: mail@ryuuneko.com (件名に `[ja-furigana security]` を付与)

報告に含めてもらえると助かる情報:

- 影響を受ける version (`furigana --version` の出力)
- 再現手順 / PoC
- 影響範囲の見立て (RCE / DoS / 情報漏洩 / 改ざん など)
- 公表希望時期があれば

### 受付後の流れ

- 7 日以内目安で受信確認 (acknowledgement) を返す
- 修正可能と判断した場合: 非公開で fix → GitHub Security Advisory + CVE 申請 (該当する場合) →
  修正版 release → reporter にクレジット (希望すれば) を付けて公開
- 受け付けない判断の場合: 理由を返す (例: 「dict TOML の内容問題は ja-furigana-dict 側で対応すべき」 など)

## 対象範囲

### In scope

- `crates/furigana` (lib): TOML loader / 形態素解析 / 読み解決パイプライン / 公開 Rust API
- `crates/furigana-cli` (bin):
  - `furigana lookup` / `furigana repl`
  - `furigana serve` (Axum HTTP server) — `/lookup` / `/admin/reload` 等
  - `furigana dict pull` (GitHub Releases から SHA-256 検証付き download → 展開)
- 公開 binary distribution (5 platform) と Docker image (`ghcr.io/ryuuneko1107/ja-furigana`)
- crates.io 上の `ja-furigana` / `ja-furigana-cli` package
- GitHub Actions workflow (CI / release / dependabot)

例えば以下のような問題は in scope:

- 信頼できない TOML を読ませると panic / unbounded memory / slow regex に陥る
- `furigana serve` の HTTP endpoint で query 文字列によって到達不能 / unbounded resource consumption
- `furigana dict pull` の SHA-256 検証バイパス、 path traversal、 archive bomb で展開先 fs を破壊
- Cargo / Docker の dependency に既知の RUSTSEC / GHSA があり、 ja-furigana 経由で利用者に影響

### Out of scope

- **辞書 ([ja-furigana-dict](https://github.com/RyuuNeko1107/ja-furigana-dict)) の内容問題**
  (誤読、 公序良俗、 著作権) → 別 repo の issue / PR で対応
- **Lindera / IPADIC が返す形態素解析の誤り** → upstream へ
- **趣味用途で意図的に同梱した依存** (例: lindera-ipadic embed-ipadic) の binary サイズや起動時間
- 利用者側の設定ミス (公開ネットワークに `furigana serve` を `--bind 0.0.0.0` で晒す等)
- 物理的に bug というより lint カテゴリの指摘 (clippy / rustfmt 単独)

## 関連窓口

- 機能要望 / 一般 bug → [GitHub Issues](https://github.com/RyuuNeko1107/ja-furigana/issues)
- 辞書追加 / 誤読 → [ja-furigana-dict Issues](https://github.com/RyuuNeko1107/ja-furigana-dict/issues)
- 依存の自動 patch (CodeQL / dependabot alerts) → maintainer が monitor 中

## クレジット (Hall of Fame)

報告を受けて確実に修正された脆弱性については、 reporter の希望があれば
GitHub Security Advisory + CHANGELOG にクレジット記載する (`Reported by @<github-handle>` の形)。
匿名希望もそのまま尊重する。
