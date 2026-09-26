# Maintaining ja-furigana

メンテナー (主に未来の自分) 向けの運用ガイド。release / publish / yank の手順、
失敗時の対応、token 管理などを記録する。

利用者向けの説明は [README.md](./README.md) を見てください。

---

## Release を打つ (binary 配布 + crates.io publish)

> **publish policy**: 0.1.0 stable (2026-05-12) 以降は release ごとに **GitHub release +
> crates.io publish** を行う (4 crate: `ja-furigana` / `ja-furigana-voicevox` /
> `ja-furigana-aquestalk` / `ja-furigana-cli`)。
> (履歴: alpha 期間中は crates.io publish を休止し GitHub release のみだった。
> 既 publish 済 alpha (alpha.1〜alpha.9) は yank しない = metadata 不変。)

### 前提
- master が緑 (CI / lint / license audit すべて pass)
- `CHANGELOG.md` の `[Unreleased]` セクションを新バージョン名にリネームし、
  日付と diff URL を追記してから commit
- 動作確認: `cargo run -p ja-furigana-cli -- lookup '灰桜の散る道' --mode ruby` が正常

### 手順

```sh
# 1. workspace の version を bump
#    ルート `Cargo.toml` の [workspace.package].version (4 crate 共通)
#    例: 0.5.0 → 0.5.1
#    crate 間の依存は `=` で完全一致 pin しているので、 次の 3 file の依存表記も合わせて更新:
#    - `crates/furigana-voicevox/Cargo.toml`  : furigana (ja-furigana) = "=0.5.X"
#    - `crates/furigana-aquestalk/Cargo.toml` : furigana (ja-furigana) = "=0.5.X"
#    - `crates/furigana-cli/Cargo.toml`       : furigana / ja-furigana-voicevox /
#                                               ja-furigana-aquestalk = "=0.5.X"

# 2. CHANGELOG.md を整理して commit
git add CHANGELOG.md Cargo.toml Cargo.lock crates/*/Cargo.toml
git commit -m "chore(release): 0.5.1"
git push origin master

# 3. tag を打って push
git tag -a v0.5.1 -m "v0.5.1 - <要約>"
git push origin v0.5.1

# 4. GitHub Actions の release workflow が走る (5 platform binary + Docker)
gh run watch --repo RyuuNeko1107/ja-furigana --workflow=release.yml

# 5. 確認
gh release view v0.5.1 --repo RyuuNeko1107/ja-furigana

# 6. crates.io にも publish (順序重要: 依存される側から)
#    lib → adapter 2 本 → cli (cli は lib + 両 adapter に `=` pin で依存)
cargo publish -p ja-furigana
# ↑ index 反映待ちで数十秒〜数分。完了を待ってから次。
cargo publish -p ja-furigana-voicevox
cargo publish -p ja-furigana-aquestalk
cargo publish -p ja-furigana-cli
```

### よくある失敗

#### `release_not_found` (binary upload で失敗)
`taiki-e/upload-rust-binary-action` は **release を作らない** (upload 専用)。
`release.yml` の `create-release` job が先頭にあるはずだが、もし古い workflow で
release が無ければ `gh release create` で空 release を先に作る:

```sh
gh release create v0.5.1 --repo RyuuNeko1107/ja-furigana \
  --target master --title v0.5.1 --generate-notes
gh run rerun <run-id> --repo RyuuNeko1107/ja-furigana --failed
```

#### tag を delete + 再 push したら workflow が trigger されない
GitHub の挙動で、同名 tag の delete + 再 push は push event として発火しない
ことがある。手動で trigger:

```sh
gh workflow run release.yml --repo RyuuNeko1107/ja-furigana -f tag=v0.5.1
```

#### `cargo fmt --check` で fail
ローカルで Windows ビルドだけ確認した時に起きやすい (CI は Linux で
fmt --check が走る)。

```sh
cargo fmt --all
git add -A && git commit -m "fix: cargo fmt"
```

#### Linux / macOS だけビルド失敗
Windows 上の `#[cfg(unix)]` でガードされたコードが Linux で動くか確認できない。
`crates/furigana-cli/src/commands/serve/mod.rs` の SIGHUP loop あたりが要注意。
ローカルで `cargo check --target x86_64-unknown-linux-gnu` (cross 必要) は
仕掛けが重いので、CI に任せて push → fail → fix のサイクルで進めて良い。

#### Docker (ghcr.io) build だけ fail (binary は通る)
Dockerfile の `FROM rust:X.Y-slim` の Rust version が依存ライブラリの
新 MSRV を満たさない時に発生。binary build は CI runner の最新 stable で走るため
通る一方、Docker は固定 version で fail する。

過去には rustyline major bump で `std::fs::File::lock` が要求され、Dockerfile の
固定 Rust version との乖離で fail した事例がある。

対処:
1. ローカルで該当依存の release notes を確認、要求 Rust version を特定
2. `Cargo.toml` workspace `rust-version` と `Dockerfile` の `FROM rust:X.Y-slim` を揃える
3. README の MSRV badge も更新
4. master に commit → 次 release のサイクルで Docker image も自動復旧
   (binary-only で release 済の場合、Docker 部分は次バージョンに持ち越して問題ない)

---

## crates.io の token 管理

### scope の使い分け

| scope | 何ができる | いつ要る |
|---|---|---|
| `publish-new` | 新規 crate の publish | 初回 publish 時 / 新 crate name 切替 |
| `publish-update` | 既存 crate の新バージョン publish | 通常の bump release 時 |
| `yank` | publish 済みバージョンを yank | 誤 publish の取り消し時 |
| `change-owners` | crate のオーナー変更 | 共同メンテナー追加時のみ |

普段使いは **`publish-new` + `publish-update` + `yank`** の 3 つを 1 つの
token に持たせると毎回切替不要。

### token 紛失時

`cargo login` した token は `~/.cargo/credentials.toml` に平文保存される。
公開環境 (CI 等) に流出した疑いがあれば即:
1. https://crates.io/me/ で該当 token を Revoke
2. 新 token を発行
3. `cargo login` しなおす

---

## yank する

```sh
cargo yank --version <version> <crate-name>
# 例 (crate 名は crates.io 上の名前 = ja- prefix 付き)
cargo yank --version 0.5.1 ja-furigana-cli
```

- yank しても crate name 自体は永久に自分が保持 (他人は取れない)。
- yank 後も既存 `Cargo.lock` 経由の DL は可能 (新規 `cargo add` だけブロック)。
- yank の取り消しは `cargo yank --undo` で可能。

---

## furigana-dict の release を CLI に反映する

`furigana dict pull` は GitHub Releases API で `ja-furigana-dict` の latest tag を
解決する。新しい辞書 release が出たら:

1. `ja-furigana-dict` 側で tag を打つ → release.yml が走って tarball + sha256 公開
2. CLI 側 (`ja-furigana-cli`) のコード変更は **不要** (latest を runtime で解決)
3. ピン留めしている利用者向けには CLI の README で `--version vYYYY.MM.DD` 例を更新 (dict は CalVer tag)

`dict_pull.rs` の `REPO` 定数を変える必要があるのは「組織名 / repo 名」が変わった時だけ
(過去に `furigana-dict` → `ja-furigana-dict` rename で必要になった)。

---

## CI / Pages / Dependabot

### CI (`ci.yml`)
- `test` (ubuntu + windows。 macOS は週次 schedule の `test-macos` のみ) / `lint` (fmt + clippy) /
  `license` (cargo-about で copyleft 検知 + NOTICE.md drift) / `audit` (cargo-audit) /
  `corpus` (ja-furigana-dict の回帰 corpus + inline `[[test]]`) / `diff-coverage`
  (変更行 coverage 80% gate) / `mutation-diff` (PR の変更行だけ cargo-mutants)
- `nightly.yml`: full mutation (8 shard 並列) + flaky 検出
- 失敗を放置せず必ず fix。fmt 違反は再 commit、clippy 違反は対応。
- license job が `about.toml` 未許可の license を検知したら、依存追加時に
  `accepted` リストに追加するか、依存を別物に切替える判断。

### Dependabot (`.github/dependabot.yml`)
- 週次 (月曜 09:00 JST) で cargo + github-actions の更新 PR が来る。
- group 化されているので関連 crate (tokio-stack / lindera / serde-stack) が
  1 PR にまとまる。CI 緑なら merge してよい。
- breaking change を含む major bump は手動レビュー必須。

### Pages (なし、削除済み)
WASM crate と一緒に削除した。再導入する場合は過去 commit (`88ee9bc` 以前) を参照。

---

## バグ / Security 報告

- 一般 bug: GitHub Issues。`bug_report.yml` テンプレが立つ。
- security: 公開 issue ではなく email (Cargo.toml の `authors` に書いてあるアドレス) に
  まずプライベートに報告してもらう。CVE が必要なら GitHub の private vulnerability
  reporting (Settings → Security → Private vulnerability reporting) を有効化。

---

## ロードマップ

[`docs/ROADMAP.md`](./docs/ROADMAP.md) を最新に保つ。完了したものは
[`CHANGELOG.md`](./CHANGELOG.md) `[Unreleased]` に移し、ROADMAP.md からは消す。
README には status の概要だけ書き、詳細は ROADMAP.md に集約する方針。
