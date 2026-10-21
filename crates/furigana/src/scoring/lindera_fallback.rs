//! Lindera fallback provider — Smart engine の safety net (★ alpha.13)。
//!
//! 詳細仕様: `docs/PROPOSALS/scoring-engine.md` §5.6 (postprocess 分離) と同 layer 観。
//!
//! ## 役割
//!
//! Smart engine の 5 provider (Protect / Alphabet / DictBridge / Number / Odoriji)
//! が一切覆わない位置 (= 助詞 / okurigana / dict 未登録 単語) を band 50 で
//! 埋めるための **最終 fallback**。
//!
//! - corpus pass で 62% uncovered (= path 構築不能) だった主因が hiragana 助詞 /
//!   okurigana の coverage 不在。 本 provider 追加で path 構築率 ≒ 100% を期待。
//! - band 50 (= `Score::lindera`) で BAND_DICT_EXACT (1000) / BAND_SPECIAL (950) /
//!   BAND_KANJI (100) より低い、 weakest_band 集約で 「他に candidate があるなら
//!   そちら勝ち、 完全に何も無い時のみ Lindera が拾う」 を実現。
//!
//! ## 設計
//!
//! - construction time に input 全体を 1 度だけ tokenize、 edge 配列を保持
//!   (= 各 `candidates_at` で再 tokenize しない、 amortized O(1) lookup)
//! - 各 [`MorphToken`] を `(byte_start, byte_end, reading)` の edge に変換、
//!   reading が無い token (= 記号 / 未知語) は surface をそのまま reading とする
//! - Lindera surface byte 累積が input byte 長と不一致なら、 edge 生成中止
//!   (defensive、 通常は Normal mode で一致するはず)
//!
//! ## band の構造 (★alpha.20 で 2 段化)
//!
//! [`crate::scoring::engine::PathScore`] は `weakest_band` (= path 中の最低 band)
//! で path を比較する。 Lindera fallback は surface 形状で band を 2 段に分ける:
//!
//! - **band 150** (= [`Score::lindera_compound`]): surface 2 字以上 + 全 char 漢字。
//!   dict 未登録の純漢字熟語 (= 最近 / 風邪 / 海外 等) を、 単漢字 default 合成
//!   (band 100 × 2) で破壊せず形態素エンジン 1 token を優先する。
//! - **band 50** (= [`Score::lindera`]): それ以外 (= 単漢字 / 漢字+okurigana 混在 /
//!   助詞 / kana のみ)。 単漢字 default + `[[kanji]]` block match の方が信頼できる
//!   ため、 最終 safety net 位置に留める。
//!
//! いずれも dict 完全一致 (1000) / 特殊処理 (950) / `[[kanji]]` block (1000) には
//! 常に負ける = dict 整備が source of truth、 Lindera は形態素 fallback 専任。
//!
//! ## 注意点
//!
//! - per-input lifetime: input を変えるたびに新しい [`LinderaFallbackProvider`]
//!   を作る必要がある (= state cache が input に依存)。 `Furigana::analyze` 内で
//!   毎回 new する想定。
//! - thread safety: 内部 [`Analyzer`] は [`std::sync::Mutex`] 保護なので、
//!   並列実行は直列化される。 single-thread 用途では問題なし。
//! - reading は Lindera 由来 = カタカナ (IPADIC details[7])、 既存 provider と
//!   出力形式整合。 Lindera reading に bracket は含まれない想定。
//!   bracket strip は Token 変換時に `parse_bracket_notation` が一括処理。

use crate::analyzer::Analyzer;
use crate::scoring::candidate::{
    CandidateProvider, RawCandidate, Score, ScoringContext, BAND_KANJI, BAND_LINDERA_UNIHAN,
};
use crate::scoring::matcher::HIT_WEIGHT_BROAD;

mod helpers;

use helpers::*;

/// band-up 対象の判定: 「CJK 統合漢字範囲のみ」 で 々/〆/ヶ は除外する。
///
/// `is_kanji_char` (= kana.rs) は 々/〆/ヶ を含むが、 これらは専用 provider
/// (OdorijiProvider / NumberCandidateProvider) が担当する範囲で、 Lindera が
/// 横取りすると path 構造が壊れる (例: 我々 → 1 token band 150 vs dict 我 +
/// 々 odoriji band 100 で前者が勝ってしまう)。 そのため band-up 判定では
/// real CJK ideograph のみ対象とする。
pub(crate) fn is_real_cjk_ideograph(c: char) -> bool {
    matches!(
        c,
        '\u{3400}'..='\u{4DBF}' |    // CJK 拡張 A
        '\u{4E00}'..='\u{9FFF}' |    // CJK 統合漢字
        '\u{F900}'..='\u{FAFF}' |    // CJK 互換
        '\u{20000}'..='\u{2A6DF}' |  // CJK 拡張 B
        '\u{2A700}'..='\u{2EBEF}' |  // CJK 拡張 C/D/E/F
        '\u{2F800}'..='\u{2FA1F}' |  // CJK 互換補助
        '\u{30000}'..='\u{323AF}'    // CJK 拡張 G/H
    )
}

/// 「読みが surface と同一」 とみなせる文字か。
///
/// - かな (ひらがな / カタカナ / 長音符 / 中黒 / 小書き)
/// - 句読点・終端記号 (。、！？…): 読み上げないが surface としてはそのまま残る
///
/// 数字 / 英字 / 漢字は **含めない** (助数詞 / loanwords / 辞書引きが要るため)。
/// 句読点・終端記号 (= 読み上げないが surface として残る文字)。
fn is_punctuation(c: char) -> bool {
    matches!(
        c,
        '。' | '、' | '！' | '？' | '…' | '!' | '?' | ',' | '.' | '・'
    )
}

fn is_standalone_kana(c: char) -> bool {
    crate::kana::is_hiragana_char(c)
        || crate::kana::is_katakana_char(c)
        || matches!(
            c,
            'ー' | '・' | '。' | '、' | '！' | '？' | '…' | '!' | '?' | ',' | '.'
        )
}

/// edge の付帯情報。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct EdgeKind {
    /// IPADIC 品詞 = 名詞/固有名詞/人名 (accent 推定用、 ADR-0007)
    is_name: bool,
    /// 漢字 1 字だけの動詞 token (= Lindera が語幹だけ切り出した形)
    verb_stem: bool,
    /// 記号だけの token の 2 文字目以降から足した edge (数 + 助数詞が token の途中で終わった時の継続用)
    symbol_suffix: bool,
}

/// 1 字動詞語幹として扱わない字。
///
/// 得 は名詞 (とく: 「ぺこ得やん」 「得すぎる」) を Lindera が一段動詞 (え) と
/// 取り違えることが多く、 A/B で改善と退行が拮抗した (★2026-09-18)。
/// 動詞用法 (得ず / 得ました) は dict の `[[kanji]]` block の送り仮名 literal で扱う。
const VERB_STEM_EXCLUDED: &[&str] = &["得"];

/// 漢字 1 字だけの surface か。
fn is_single_kanji(surface: &str) -> bool {
    let mut chars = surface.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if is_real_cjk_ideograph(c))
}

/// Lindera tokenize 結果を edge 配列で保持する fallback provider。
///
/// construction 時に 1 度だけ tokenize、 以降 `candidates_at` は start 昇順の edge 配列を
/// 二分探索して位置 lookup (O(log edge_count))。
#[derive(Debug, Clone, Default)]
pub struct LinderaFallbackProvider {
    /// (byte_start, byte_end, reading, kind) の 4-tuple。
    /// reading は カタカナ (Lindera 由来) または surface fallback。
    /// kind は品詞由来の付帯情報 ([`EdgeKind`])。
    edges: Vec<(usize, usize, String, EdgeKind)>,
    /// 形態素 token の (start, end) (start 昇順)。 行き止まり補完で 「pos を含む token」 を引く。
    tokens: Vec<(usize, usize)>,
}

impl LinderaFallbackProvider {
    /// `analyzer` で `input` を tokenize、 edge 配列を構築。
    ///
    /// Lindera tokenize 失敗 / surface byte 不一致 時は edges 空で初期化
    /// (= provider 自体が無効、 caller path に影響なし)。
    #[must_use]
    pub fn new(analyzer: &Analyzer, input: &str) -> Self {
        if input.is_empty() {
            return Self::default();
        }
        let tokens = analyzer.tokenize_light(input);
        let mut edges = Vec::with_capacity(tokens.len());
        let mut byte_pos = 0usize;
        let attaches_next: Vec<bool> = tokens
            .iter()
            .skip(1)
            .map(|t| t.attaches_to_verb)
            .chain(std::iter::once(false))
            .collect();
        // 直後が 「に + 移動の動詞」 (食いに行く / 刺しにきてる) = 動詞の連用形で、 接尾の連濁形ではない
        let next_ni_motion: Vec<bool> = (0..tokens.len())
            .map(|i| {
                tokens
                    .get(i + 1)
                    .is_some_and(|t| t.is_particle && t.surface == "に")
                    && tokens.get(i + 2).is_some_and(|t| {
                        t.surface.starts_with(['行', 'い', '来', 'き', 'く', 'こ'])
                    })
            })
            .collect();
        // 直前の語が連濁を起こさない境界か (文頭 / 空白・記号 / 助詞 / 接頭詞 お・ご /
        // ひらがなで終わる未知語 = 「ぺこらの」 「ぺこちゃんお」 のような読み無し token)
        let mut prev_is_boundary = true;
        // 上のうち、 閉じ括弧とひらがなの未知語を除いた強い境界。 通り / 沿い 等は閉じ括弧や未知語の後ろだと
        // 名詞の続き (「TODO.md」通り / 国道４１号）沿い / やちむん通り) なので、 強い境界の直後だけ清音に戻す
        let mut prev_is_strong_boundary = true;
        // 直前の token がカタカナ語 (ロング / ユミン) か。 カタカナ語 + 使い は つかい (ロング使い = ロングの使い手)
        let mut prev_is_katakana = false;
        for ((tok, next_attaches), ni_motion) in
            tokens.into_iter().zip(attaches_next).zip(next_ni_motion)
        {
            let surface_len = tok.surface.len();
            // Lindera は空白 / 改行 / 制御文字を token から落とすことがある
            // (例: input に `\n` / `( ・∇・)` の半角 space)。 落ちると byte_pos が
            // surface 開始位置とズレるので、 input 中で surface を前方探索して
            // アラインを回復し、 間に挟まった bytes を passthrough edge で補う。
            // こうしないと改行位置が誰にも覆われず dp[n] 未到達 → path 全空
            // (= 改行入り input で出力が空になる bug)。
            if input.get(byte_pos..byte_pos + surface_len) != Some(tok.surface.as_str()) {
                let Some(rel) = input[byte_pos..].find(tok.surface.as_str()) else {
                    // surface が前方にも無い (= 想定外のズレ) → safety net disable
                    return Self::default();
                };
                let gap_end = byte_pos + rel;
                if !Self::push_skipped_gap(input, byte_pos, gap_end, &mut edges) {
                    return Self::default();
                }
                byte_pos = gap_end;
                prev_is_boundary = true;
                prev_is_strong_boundary = true;
                prev_is_katakana = false;
            }
            let end = byte_pos + surface_len;
            // reading: Lindera details[7] (= カタカナ)、 無ければ surface fallback
            // (= 記号 / 未知語、 reading = surface で 「読まない」 扱い)
            let kind = EdgeKind {
                is_name: tok.is_person_name,
                // 送り仮名なしで 1 字になる動詞語幹は一段動詞 (寝る / 出る / 見る) に限られる。
                // 熟語内の 1 字を動詞と誤判定する例 (逆|走 / 暗|視 / お|得) を避けるため、
                // 直後が動詞に後接する語で、 直前が漢字・接頭の お/ご でない時だけ。
                verb_stem: tok.is_ichidan_verb
                    && next_attaches
                    && is_single_kanji(&tok.surface)
                    && !VERB_STEM_EXCLUDED.contains(&tok.surface.as_str())
                    && !input[..byte_pos].chars().next_back().is_some_and(|c| {
                        is_real_cjk_ideograph(c) || matches!(c, 'お' | 'ご' | '御')
                    }),
                symbol_suffix: false,
            };
            // 代名詞 (これ帰り / それ使い) の後ろでも複合語の連濁は起きない (2026-09-27)
            let this_is_boundary = tok.is_particle
                || tok.is_pronoun
                || (tok.is_prefix && matches!(tok.surface.as_str(), "お" | "ご" | "御"))
                || tok.surface.chars().all(is_rendaku_boundary_symbol)
                || (tok.reading.is_none()
                    && tok.surface.chars().next_back().is_some_and(|c| {
                        crate::kana::is_hiragana_char(c)
                            && !matches!(c, 'ぁ' | 'ぃ' | 'ぅ' | 'ぇ' | 'ぉ' | 'っ')
                    }));
            let this_is_strong_boundary = tok.is_particle
                || tok.is_pronoun
                || (tok.is_prefix && matches!(tok.surface.as_str(), "お" | "ご" | "御"))
                || (tok.surface.chars().all(is_rendaku_boundary_symbol)
                    && !tok
                        .surface
                        .chars()
                        .next_back()
                        .is_some_and(is_closing_symbol));
            let mut reading = tok.reading.unwrap_or_else(|| tok.surface.clone());
            // IPADIC は 疲れ=ヅカレ / 使い=ヅカイ / 書き=ガキ 等の連濁形を 名詞-接尾 として持ち、
            // 文頭や助詞の直後 (書きはムリ / ぺこらの勝ち / お疲れ) にも付けてしまう。
            // 連濁は前に語がある時だけ起きるので、 境界の直後と 「に + 移動動詞」 の前では清音に戻す
            // 通り / 帰り 等は名詞の後ろで濁るのが普通なので 「に + 移動動詞」 の判定からは外すが、
            // 境界の直後 (文頭 / 助詞 / 代名詞 = これ帰り) では他の語と同じく清音に戻す (2026-09-27)
            let after_boundary = if is_okurigana_form_voiced_after_noun_excluded(&tok.surface) {
                prev_is_boundary
            } else {
                prev_is_strong_boundary
            };
            // カタカナ語の直後の 使い (ロング使い / ユミン使い / デコ使い) は つかい。
            // ヅカイ (人使い / 言葉使い / 魔物使い) は漢字の語の後ろの連濁
            let katakana_tsukai = prev_is_katakana && tok.surface == "使い";
            if tok.is_noun_suffix
                && ((after_boundary && is_okurigana_form(&tok.surface))
                    || (ni_motion && is_okurigana_form_voiced_after_noun_excluded(&tok.surface))
                    || katakana_tsukai)
            {
                if let Some(plain) = devoice_first_kana(&reading) {
                    reading = plain;
                }
            }
            prev_is_boundary = this_is_boundary;
            prev_is_strong_boundary = this_is_strong_boundary;
            prev_is_katakana = !tok.surface.is_empty()
                && tok
                    .surface
                    .chars()
                    .all(|c| crate::kana::is_katakana_char(c) || c == 'ー');
            edges.push((byte_pos, end, reading, kind));
            byte_pos = end;
        }
        // 末尾に Lindera が落とした空白 / 改行が残る場合も passthrough で補う
        // (= 「猫\n」 のような trailing newline で path 全空になるのを防ぐ)。
        if byte_pos < input.len()
            && !Self::push_skipped_gap(input, byte_pos, input.len(), &mut edges)
        {
            return Self::default();
        }
        let tokens: Vec<(usize, usize)> = edges.iter().map(|(s, e, _, _)| (*s, *e)).collect();
        Self::push_kana_suffix_edges(input, &mut edges);
        // `candidates_at` が start 位置で二分探索できるよう start 昇順に並べる。
        // **安定** sort なので、 同じ start の edge 同士の順序 (= 候補の列挙順、
        // Viterbi の同点 tie-break に効く) は push 順のまま保たれる。
        edges.sort_by_key(|(start, _, _, _)| *start);
        Self { edges, tokens }
    }

    /// 空 provider (test 用、 input なしで安全に new する)。
    #[allow(dead_code)] // test utility; kept for external callers and future use
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// かな token の **途中から始まる suffix edge** を補う。
    ///
    /// ## なぜ要るか
    ///
    /// edge は Lindera の token 境界からしか生えないため、 dict entry が token の
    /// 途中で終わると **その先に繋げる edge が 1 本も無く、 entry を含む path が
    /// DP 上そもそも構築できない**。 実例:
    ///
    /// ```text
    /// 「お婆ちゃんすげぇ」 → Lindera: お / 婆 / ち / ゃんすげぇ
    /// entry 「お婆ちゃん」 (band 1000) は 0..12 を覆うが、 12 (= 「す」) から
    /// 始まる edge が無いので採択できず 「おばばちゃんすげぇ」 になる。
    /// ```
    ///
    /// 「お婆ちゃんやばい」 が正しく読めるのは、 たまたま Lindera が
    /// 「お婆ちゃん」 で切ってくれるから。 つまり **辞書を足しても効かない位置**が
    /// 入力次第で発生する。
    ///
    /// ## 何を足すか
    ///
    /// かな (ひらがな / カタカナ / 長音符 / 中黒) だけで構成された token について、
    /// 2 文字目以降の各 char 境界から token 末尾までの edge を足す。 かなは
    /// 読みが surface と同一なので reading は substring をそのまま使える。
    /// 漢字を含む token には足さない (読みを機械的に切れないため)。
    ///
    /// band は元 token と同じ [`Score::lindera`] なので、 既存 path の
    /// `weakest_band` は下がらない。 edge 数は増えるが `edge_count` は
    /// 「少ない方が良い」 軸なので、 必要な時 (= 高 band entry の継続) にだけ選ばれる。
    fn push_kana_suffix_edges(input: &str, edges: &mut Vec<(usize, usize, String, EdgeKind)>) {
        let base: Vec<(usize, usize)> = edges.iter().map(|(s, e, _, _)| (*s, *e)).collect();
        for (start, end) in base {
            let Some(surface) = input.get(start..end) else {
                continue;
            };
            if surface.chars().count() < 2 {
                continue;
            }
            // % で始まる記号だけの token (「%→」 「%)」 「%/」) は、 2 文字目以降から始まる edge も足す。 Lindera が記号を 1 つにまとめると、
            // 数 + 助数詞の候補 (「55%」) が token の途中で終わり、 次に繋ぐ edge が無くて採択できない (55%→60% の % が読まれない)
            if surface.starts_with(['%', '％']) && surface.chars().all(is_split_symbol) {
                for (offset, _) in surface.char_indices().skip(1) {
                    let sub_start = start + offset;
                    edges.push((
                        sub_start,
                        end,
                        input[sub_start..end].to_string(),
                        EdgeKind {
                            symbol_suffix: true,
                            ..EdgeKind::default()
                        },
                    ));
                }
                continue;
            }
            // token 末尾の **かな連続部分** を求める (「守りの」 → 「りの」、 「すげぇ」 → 全体)。
            let Some(tail_start) = surface
                .char_indices()
                .rev()
                .take_while(|(_, c)| is_standalone_kana(*c))
                .last()
                .map(|(i, _)| i)
            else {
                continue;
            };
            // **かな tail の 2 文字目以降** から末尾までを足す。
            //
            // 先頭 (= tail_start 自身) を含めないのが要点。 含めると 「35点目指そう」 で
            // 「35点目 (counter) + 指 + そう」 という path が成立し、 目 を助数詞として
            // 食って 「さんじゅうごてんめさそう」 になる (★2026-09-11 A/B で検出)。
            // dict entry の継続に要るのは 「entry が途中で終わった次の位置」 なので、
            // tail の 2 文字目以降で足りる (「守り|の」 の の、 「含羞む|で」 の で)。
            for (offset, ch) in surface[tail_start..].char_indices() {
                // tail の先頭は原則 skip (送り仮名を切ると 「35点目指そう」 が
                // 「35点目 + 指 + そう」 に割れるため)。 例外は 2 つ:
                //
                // 1. **句読点・終端記号** (助数詞の材料にならない。 「50%。」 で 。 の
                //    edge が無く 「%」 が読まれなかった)
                // 2. **助詞 1 文字だけの tail** (IPADIC が 「目と」 「円か」 を 1 語に持つため
                //    1つ目 + と / 1億円 + か の path が組めず いちつもく / いちおくまど に
                //    なっていた。 助詞は dict entry の読みに含まれないので、 送り仮名を
                //    切った時の 「藍染め = あいぞめめ」 型の重複は起きない)
                let single_particle_tail = surface[tail_start..].chars().count() == 1
                    && matches!(
                        ch,
                        // 'の' は除く: 「100万分の1」 の 分の (分数) を壊す
                        'は' | 'が' | 'を' | 'に' | 'と' | 'も' | 'で' | 'や' | 'か'
                    );
                if offset == 0 && !is_punctuation(ch) && !single_particle_tail {
                    continue;
                }
                let sub_start = start + tail_start + offset;
                if sub_start == start {
                    continue;
                }
                edges.push((
                    sub_start,
                    end,
                    input[sub_start..end].to_string(),
                    EdgeKind::default(),
                ));
            }
        }
    }

    /// Lindera が落とした `input[start..end]` 区間を passthrough edge で補う。
    ///
    /// 区間が空白 / 制御文字のみ (= Lindera が token から落とす種類) なら、
    /// reading = surface の passthrough edge を 1 本足して `true`。 それ以外
    /// (= 実 surface のズレ、 構造的問題) は edge を足さず `false` を返し、
    /// caller に safety net 全廃 (disable) を促す。 空区間は no-op で `true`。
    fn push_skipped_gap(
        input: &str,
        start: usize,
        end: usize,
        edges: &mut Vec<(usize, usize, String, EdgeKind)>,
    ) -> bool {
        if start >= end {
            return true;
        }
        let gap = &input[start..end];
        // Lindera が落とした区間は **中身を問わず** passthrough edge で埋める。
        //
        // 以前は空白 / 制御文字のみ埋め、 それ以外は `false` を返して safety net 自体を
        // 無効化していた。 その結果 **Lindera が落とす 1 文字 (例: Ð U+00D0) が入るだけで
        // 入力全体の読みが空になっていた** (「応援曲個人設定できるんÐな」 → 空文字列。
        // ★2026-09-11 実コーパス 704 万行の走査で検出)。
        // 読めない文字は surface のまま残す方が、 文全体を失うより常に良い。
        edges.push((start, end, gap.to_string(), EdgeKind::default()));
        true
    }
}

impl CandidateProvider for LinderaFallbackProvider {
    /// 行き止まり補完: `pos` を **途中に含む** 形態素 token の残り `input[pos..end]` が
    /// かな (+ 句読点) だけなら、 それを 1 本の edge として足す。
    ///
    /// [`Self::push_kana_suffix_edges`] は tail の先頭を原則 skip する (35点目|指|そう 対策)。
    /// ここは engine が 「authored edge がここで終わり、 候補が無い」 と確認した位置でしか
    /// 呼ばれないので、 tail 先頭 (断捨離|し / 介|さ / お腹|いっぱい) も足してよい。
    /// 漢字を含む残りは足さない (読みを機械的に切れない)。
    fn dead_end_candidates_at<'b>(
        &'b self,
        ctx: &ScoringContext<'b>,
        pos: usize,
        out: &mut Vec<RawCandidate<'b>>,
    ) {
        let input = ctx.input;
        let i = self.tokens.partition_point(|(start, _)| *start < pos);
        let Some(&(start, end)) = i.checked_sub(1).and_then(|i| self.tokens.get(i)) else {
            return;
        };
        if !(start < pos && pos < end) {
            return;
        }
        let Some(rest) = input.get(pos..end) else {
            return;
        };
        // かな (+ 句読点) か、 記号だけの残り ((^o^)↑ の ↑ のように、 顔文字などの保護 token の直後で
        // 形態素が 「^)↑」 と 1 つにしてしまった場合)。 記号は表記のまま通す
        let symbol_only = |c: char| !c.is_alphanumeric() && !c.is_whitespace() && !c.is_control();
        if rest.is_empty()
            || !rest
                .chars()
                .all(|c| is_standalone_kana(c) || symbol_only(c))
        {
            return;
        }
        let length = u8::try_from(rest.chars().count()).unwrap_or(u8::MAX);
        out.push(RawCandidate::new(rest, pos..end, Score::lindera(length)));
    }

    fn candidates_at<'b>(
        &'b self,
        ctx: &ScoringContext<'b>,
        pos: usize,
        out: &mut Vec<RawCandidate<'b>>,
    ) {
        let input = ctx.input;
        // edges は start 昇順 (new で安定 sort 済) = start == pos の edge は連続区間。
        // 旧実装は全 edge を毎位置 filter していた (入力長に対して O(N²))。
        let lo = self.edges.partition_point(|(start, _, _, _)| *start < pos);
        let hi = lo + self.edges[lo..].partition_point(|(start, _, _, _)| *start == pos);
        let found = self.edges[lo..hi]
            .iter()
            .map(|(start, end, reading, kind)| {
                let surface = &input[*start..*end];
                let char_count = surface.chars().count();
                let length = u8::try_from(char_count).unwrap_or(u8::MAX);
                // ★alpha.20: 「2 字以上 + 全 char 漢字」 の surface に限り band 150 に
                // 格上げ。 dict 未登録の純漢字熟語 (= 最近 / 風邪 / 海外 等) で 単漢字
                // default 合成 (band 100 × 2) より形態素 1 token を優先する。
                //
                // 単漢字 (例: 私) は 50 のまま (= overrides.toml の `[[kanji]]` context
                // match で制御)、 漢字+okurigana 混在 (例: 来た) も 50 のまま
                // (= `[[kanji]]` block の `next_char_type` match で declarative 解決)。
                let score = if kind.verb_stem {
                    // 1 字だけ切り出された動詞 (寝|ん / 出|ん / 見|合わ) は Lindera の
                    // 活用込みの読み (ネ / デ / ミ) を単漢字 block と同じ band で出す。
                    // match_hits 1 = [[kanji]] block の broad hit と同点 (列挙順で dict が勝つ)、
                    // literal hit (= 送り仮名 literal 等、 2) なら dict が勝つ。
                    // default しか当たらない単漢字 (寝 = シン) には Lindera が勝つ。
                    Score::new(BAND_KANJI, length, HIT_WEIGHT_BROAD)
                } else if kind.symbol_suffix {
                    // 記号の途中から足した edge: 同点の経路 (55%→ を 55 + %→ と読むか 55% + → と読むか) では、
                    // 数 + 助数詞が % まで読める側を選ぶ。 band は Lindera と同じで、 同点の時だけ効く
                    Score::new(BAND_LINDERA_UNIHAN, length, HIT_WEIGHT_BROAD)
                } else if char_count >= 2 && surface.chars().all(is_real_cjk_ideograph) {
                    Score::lindera_compound(length)
                } else {
                    Score::lindera(length)
                };
                RawCandidate::new(reading.as_str(), *start..*end, score)
                    .with_name_flag(kind.is_name)
            });
        out.extend(found);
    }
}

#[cfg(test)]
mod tests;
