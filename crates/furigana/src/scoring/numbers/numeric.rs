//! 慣用語句・漢数字・分数・素の数字など数そのものの matcher
//!
//! `NumberCandidateProvider::candidates_at` (mod.rs) が適用順を持ち、 ここは各 `try_*` の中身だけ。

use super::context::after_dai;
use super::context::kanji_digit;
use super::context::positional_digits;
use super::context::two_digit_positional;
use super::patterns::{
    at_start, DIGIT_RE, FRACTION_RE, KANJI_DECIMAL_AFTER_BUN_RE, KANJI_DECIMAL_RE,
    KANJI_DECIMAL_SHORT_RE, KANJI_DECIMAL_UNIT_RE,
};
use super::NumberCandidateProvider;
use crate::numbers::number_to_katakana;
use crate::scoring::candidate::RawCandidate;

impl NumberCandidateProvider {
    /// section 0: 数詞慣用語句 (二十歳=ハタチ / 明後日=アサッテ 等)。
    ///
    /// 「明後日」 のように数字以外の先頭を持つ語句もあるため、 caller は numeric lead
    /// 判定より **前に** 呼ぶこと。 同位置で複数 hit (= 「一人前」 と 「一人」) は
    /// 全部 emit し、 採択は DP の length / edge_count 軸に委ねる (長い方が勝つ)。
    /// dict 完全一致 (band 1000) には負ける = dict 側で個別 override 可能。
    pub(super) fn try_phrase(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        first_char: char,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
        let Some(bucket) = self.phrase_index.get(&first_char) else {
            return;
        };
        for (surface, reading) in bucket {
            if rest.starts_with(surface.as_str()) {
                out.push(self.make(input, pos, surface.len(), reading.clone()));
            }
        }
    }

    /// 助数詞の付かない漢数字の列 (七百七十三 / 千七百 / 百九億)。 2026-09-27
    ///
    /// 従来は 1 字ずつ dict の漢字規則で読まれ、 助数詞向けの促音規則が数の中に当たっていた
    /// (七百 = しっぴゃく / 百九 = ひゃっきゅう / 七千 = しちせん)。 数として読む候補を出す。
    /// 対象は 2 字以上で、 位の字 (十百千万億) を含むか 〇 を含む位取り 3 桁以上のもの。
    /// 先頭が 万 / 億 (万一)、 数字が 2 つ続く概数 (三十七八 / 八九)、 漢数字の列の途中、
    /// 直後が漢字 (助数詞: 十一兆 / 十二月 / 第十一項 は既存の促音・特殊読みに任せる) の時は出さない
    pub(super) fn try_kanji_bare_number(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
        const DIGITS: &str = "〇一二三四五六七八九";
        const UNITS: &str = "十百千万億";
        let run: String = rest
            .chars()
            .take_while(|&c| DIGITS.contains(c) || UNITS.contains(c))
            .collect();
        let chars: Vec<char> = run.chars().collect();
        if chars.len() < 2 || matches!(chars[0], '万' | '億') {
            return;
        }
        // 数の形をした慣用語は形態素解析の読みに任せる (八百万 = やおよろず)
        if matches!(run.as_str(), "八百万") {
            return;
        }
        if input[..pos]
            .chars()
            .next_back()
            .is_some_and(|c| DIGITS.contains(c) || UNITS.contains(c))
        {
            return;
        }
        let has_unit = chars.iter().any(|&c| UNITS.contains(c));
        // 2 桁は カタカナに挟まった字 (ラ三一ヌ) を除く。 同じ字の繰り返し (⊂二二 / 三三 / 八八ノ = 顔文字・効果線) は
        // 第一一 / （一一） / 一一―四 / 二二キロワット のように 第 の後・括弧で閉じた番号・ダッシュの前後・単位 (キロ / GHz) の前だけ数にする。
        // 助数詞の付く形 (二二年 / 三三％) は counter 側で読む
        let dai = after_dai(input, pos);
        let after = &rest[run.len()..];
        let numeric_repeat = dai
            || after.starts_with(['―', '－', '-'])
            || input[..pos].ends_with(['―', '－', '-'])
            || after.starts_with(|c: char| c.is_ascii_alphabetic())
            || (after.starts_with(['）', ')']) && input[..pos].ends_with(['（', '(']))
            || [
                "キロ",
                "メートル",
                "ミリ",
                "センチ",
                "グラム",
                "トン",
                "ワット",
                "ボルト",
                "ヘルツ",
                "パーセント",
            ]
            .iter()
            .any(|u| after.starts_with(u));
        let two_digit = two_digit_positional(&run, dai)
            && (chars[0] != chars[1] || numeric_repeat)
            && !input[..pos]
                .chars()
                .next_back()
                .is_some_and(crate::kana::is_katakana_char);
        // 3 桁以上で英字の単位が続く (一四二MHz / 二一五ｋＨｚ) なら 〇 が無くても位取り。 2026-09-29
        let before_latin = rest[run.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, 'Ａ'..='Ｚ' | 'ａ'..='ｚ'));
        let positional = !has_unit
            && ((chars.len() >= 3 && (chars.contains(&'〇') || before_latin)) || two_digit);
        if !(has_unit || positional) {
            return;
        }
        let consecutive_digits = chars.windows(2).any(|w| {
            w[0] != '〇' && w[1] != '〇' && DIGITS.contains(w[0]) && DIGITS.contains(w[1])
        });
        if has_unit && consecutive_digits {
            return;
        }
        // 同じ位置からもっと長い候補 (助数詞付き: 十一兆 / 十二月 / 三十八分 / 辞書の 八百屋) が既にあれば出さない。
        // 数だけの候補が 助数詞の促音・特殊読み (じゅういっちょう / じゅうにがつ) に勝ってしまうため
        if out
            .iter()
            .any(|c| c.range.start == pos && c.range.end >= pos + run.len())
        {
            return;
        }
        // 直後が漢字なら助数詞 (年 / 兆 / 項 / 号 …) として既存の読み (促音・特殊読み: じゅういっちょう /
        // じゅうにがつ / じゅういっこう) に任せる。 後ろがかな・カタカナ・記号・行末の数だけを対象にする
        // ただし 〇 を含む位取り (一〇以上 / 二〇平方メートル) は 1 字ずつ読むと いちれい になるだけなので出す。 2026-09-29
        // 型式 (九〇式 = きゅうまるしき) / 〇 始まり / 小数部 (秒・〇八三〇東経) は従来どおり
        let next = rest[run.len()..].chars().next();
        let zero_positional = positional
            && chars.contains(&'〇')
            && chars[0] != '〇'
            && next != Some('式')
            && !input[..pos].ends_with(['・', '．', '.']);
        if !zero_positional && next.is_some_and(crate::kana::is_kanji_char) {
            return;
        }
        let arabic = if two_digit {
            positional_digits(&run)
        } else {
            crate::numbers::helpers::kansuji_to_arabic(&run)
        };
        let Some(arabic) = arabic else {
            return;
        };
        out.push(self.make(input, pos, run.len(), number_to_katakana(&arabic)));
    }

    /// 位取りの漢数字の小数 (一・〇〇七 = イチテンゼロゼロナナ / 〇・五〇 = ゼロテンゴゼロ)。
    /// 漢数字の列の途中 (直前が漢数字 / ・) からは出さない。 2026-09-27
    pub(super) fn try_kanji_decimal(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
        let Some(caps) = at_start(&KANJI_DECIMAL_RE, rest)
            .or_else(|| {
                // 小数部 1 桁 (三八・四％) は 整数部が位取り 2 桁の時だけ。 一・二 / 三・四 は並列の列挙
                at_start(&KANJI_DECIMAL_SHORT_RE, rest)
                    .filter(|c| two_digit_positional(c.get(1).unwrap().as_str(), false))
            })
            .or_else(|| {
                // 十分の七・五 / 百分の二・五: 「〜分の」 の直後は単位が無くても小数 (法令の割合)
                if input[..pos].ends_with("分の") {
                    at_start(&KANJI_DECIMAL_AFTER_BUN_RE, rest)
                } else {
                    None
                }
            })
            .or_else(|| {
                // 1 桁・1 桁でも、 直後に単位 (％ / 倍 / 万円 …) が付けば小数 (二・五％ / 一・八倍)。
                // 三・四 のように 1 つ違いで上がる並びは 並列・範囲 (3 と 4) の可能性が高いので除く (〇・一 は小数)
                at_start(&KANJI_DECIMAL_UNIT_RE, rest).filter(|c| {
                    let d = |i: usize| {
                        c.get(i)
                            .unwrap()
                            .as_str()
                            .chars()
                            .next()
                            .and_then(kanji_digit)
                            .and_then(|x| x.to_digit(10))
                    };
                    match (d(1), d(2)) {
                        (Some(a), Some(b)) => a == 0 || b != a + 1,
                        _ => false,
                    }
                })
            })
        else {
            return;
        };
        if input[..pos]
            .chars()
            .next_back()
            .is_some_and(|c| c == '・' || kanji_digit(c).is_some())
        {
            return;
        }
        // 数の終わり (単位つきの形では単位は数に含めない)
        let m_end = caps.get(2).unwrap().end();
        let to_arabic = |s: &str| s.chars().filter_map(kanji_digit).collect::<String>();
        let num = format!(
            "{}.{}",
            to_arabic(caps.get(1).unwrap().as_str()),
            to_arabic(caps.get(2).unwrap().as_str())
        );
        out.push(self.make(input, pos, m_end, number_to_katakana(&num)));
    }

    /// section 5.5: 分数 「N分のM」 (間の半角空白は任意)。 「3分の1」 は従来も 分の = ぶんの だったが、
    /// それは 「3分」 (さんぷん) の後の 「の」 に候補が無く行き止まりになる偶然だった。 明示の候補にする
    pub(super) fn try_fraction(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
        if let Some(caps) = at_start(&FRACTION_RE, rest) {
            let m_end = caps.get(0).unwrap().end();
            let den = caps.get(1).unwrap().as_str();
            let num = caps.get(2).unwrap().as_str();
            let reading = format!(
                "{}ブンノ{}",
                number_to_katakana(den),
                number_to_katakana(num)
            );
            out.push(self.make(input, pos, m_end, reading));
        }
    }

    /// 数字なしの `km/h` / `m/s` (units に `/` 付きで登録した単位)。 語頭で、 直後が英数字でない時だけ。
    /// 数字つき (60km/h) は si_unit の regex が読む
    pub(super) fn try_slash_unit(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        first_char: char,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
        if self.slash_units.is_empty() || !first_char.is_ascii_alphabetic() {
            return;
        }
        // URL / パス / メールの一部 (a.com/km/h、 ?v=m/s、 foo@m/s) は読まない
        if input[..pos].chars().next_back().is_some_and(|c| {
            c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    '/' | '.' | '_' | '-' | ':' | '@' | '=' | '?' | '&' | '%' | '#' | '~'
                )
        }) {
            return;
        }
        for key in &self.slash_units {
            let Some(head) = rest.get(..key.len()) else {
                continue;
            };
            if !head.eq_ignore_ascii_case(key) {
                continue;
            }
            if rest[key.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-'))
            {
                continue;
            }
            if let Some(kana) = self.units.lookup(head) {
                out.push(self.make_hit(input, pos, key.len(), kana.to_string()));
                return;
            }
        }
    }

    /// section 8: 素の数字。
    pub(super) fn try_digit(
        &self,
        input: &str,
        pos: usize,
        rest: &str,
        out: &mut Vec<RawCandidate<'_>>,
    ) {
        if let Some(m) = at_start(&DIGIT_RE, rest) {
            let m_end = m.get(0).unwrap().end();
            let num = m.get(0).unwrap().as_str();
            // 8 が 3 つ以上並ぶだけの `888` / `88888` は拍手 (パチパチ)。 数として読むと 「はっぴゃく…」 になる。
            // 前後が数字・英字・漢字・小数点・位取りの区切りではない時だけ (888円 は助数詞の候補が読む)
            if num.len() >= 3
                && num.bytes().all(|b| b == b'8')
                && !input[..pos]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || matches!(c, '.' | ',' | '，'))
                && !rest[m_end..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric() || matches!(c, '.' | ',' | '，'))
            {
                out.push(self.make_hit(input, pos, m_end, "パチ".repeat(num.len())));
                return;
            }
            out.push(self.make(input, pos, m_end, number_to_katakana(num)));
        }
    }
}
