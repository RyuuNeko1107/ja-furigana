//! 形態素解析器 (Lindera + IPADIC ラッパ)
//!
//! [`Analyzer::new`] で IPADIC を埋め込みロードし、`tokenize` で
//! テキストを [`MorphToken`] 列に分解する。
//!
//! `Tokenizer::tokenize_with_lattice` は `&self` で呼べる (辞書は読み取り専用) ので、 tokenizer は
//! スレッド間で共有し、 解析ごとの作業領域 (lattice) だけを小さなプールから借りる。
//! 複数スレッドから同時に解析できる (並列実行)。

use crate::error::{FuriganaError, Result};
use lindera::dictionary::load_dictionary;
use lindera::dictionary::Dictionary;
use lindera::dictionary::Lattice;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use lindera::tokenizer::Tokenizer;
use std::sync::Mutex;

/// 形態素解析の 1 トークン
///
/// IPADIC の details 配列から `*` または空文字を `None` に正規化済み。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MorphToken {
    /// 表層形
    pub surface: String,
    /// カタカナ読み (IPADIC details\[7\])
    pub reading: Option<String>,
    /// 品詞 (IPADIC details\[0\]) — 名詞 / 動詞 / 形容詞 等
    pub pos: Option<String>,
    /// 品詞細分類 1 (IPADIC details\[1\]) — 普通名詞 / 固有名詞 / 数 等
    pub pos_detail: Option<String>,
    /// 品詞細分類 2 (IPADIC details\[2\]) — 人名 / 地域 / 組織 等 (固有名詞の下位分類)
    pub pos_detail2: Option<String>,
    /// 品詞細分類 3 (IPADIC details\[3\]) — 姓 / 名 等 (人名の下位分類)
    pub pos_detail3: Option<String>,
    /// 活用型 (IPADIC details\[4\]) — 五段・カ行イ音便 / 一段 等
    pub conjugation_type: Option<String>,
    /// 活用形 (IPADIC details\[5\]) — 基本形 / 連用形 等
    pub conjugation_form: Option<String>,
    /// 原形 (IPADIC details\[6\]) — 「食べた」→「食べる」
    pub base_form: Option<String>,
}

impl MorphToken {
    /// surface だけセットしたトークンを返す (フォールバック用)
    fn surface_only(text: &str) -> Self {
        Self {
            surface: text.to_string(),
            reading: None,
            pos: None,
            pos_detail: None,
            pos_detail2: None,
            pos_detail3: None,
            conjugation_type: None,
            conjugation_form: None,
            base_form: None,
        }
    }
}

/// 形態素解析器
pub struct Analyzer {
    /// 読み取り専用 (解析中に書き換わらない)。 全スレッドで共有する
    tokenizer: Tokenizer,
    /// 使い回す lattice の置き場。 借りる・返す一瞬だけ lock を取る (解析中は持たない)
    lattices: Mutex<Vec<Lattice>>,
}

/// 返却を受け付ける lattice の最大数 (同時解析数の目安。 超えた分は捨てる)
const LATTICE_POOL_MAX: usize = 64;

/// これより長い入力を解析した後は lattice を捨てる (byte 数)。
///
/// lattice の容量は過去最長の入力に合わせて伸びたまま残るので、 巨大入力 1 回で
/// 常駐メモリが増え続けないよう、 閾値超えの時だけ作り直す。
const LATTICE_RETAIN_MAX_BYTES: usize = 64 * 1024;

/// 埋め込み辞書の URI ★alpha.17。 feature flag (= `dict-ipadic` / `dict-unidic`) で
/// 排他的に switch。 IPADIC と UniDic で details field の意味が違うので、
/// reading / base_form の field 番号も合わせて切り替える。
#[cfg(all(feature = "dict-ipadic", feature = "dict-unidic"))]
compile_error!("Enable exactly one of `dict-ipadic` / `dict-unidic`, not both.");

#[cfg(not(any(feature = "dict-ipadic", feature = "dict-unidic")))]
compile_error!("Enable exactly one of `dict-ipadic` (default) / `dict-unidic` features.");

#[cfg(all(feature = "dict-ipadic", not(feature = "dict-unidic")))]
const EMBEDDED_DICT_URI: &str = "embedded://ipadic";
#[cfg(all(feature = "dict-unidic", not(feature = "dict-ipadic")))]
const EMBEDDED_DICT_URI: &str = "embedded://unidic";

/// details field 番号: reading (= 表層形のカタカナ発音)。
///
/// - IPADIC: details\[7\] (カタカナ reading)
/// - UniDic: details\[9\] (pron = 発音形出現形)
#[cfg(all(feature = "dict-ipadic", not(feature = "dict-unidic")))]
const FIELD_READING: usize = 7;
#[cfg(all(feature = "dict-unidic", not(feature = "dict-ipadic")))]
const FIELD_READING: usize = 9;

/// details field 番号: base_form (= 原形 / 辞書形)。
///
/// - IPADIC: details\[6\] (原形)
/// - UniDic: details[10] (orthBase = 書字形基本形)
#[cfg(all(feature = "dict-ipadic", not(feature = "dict-unidic")))]
const FIELD_BASE_FORM: usize = 6;
#[cfg(all(feature = "dict-unidic", not(feature = "dict-ipadic")))]
const FIELD_BASE_FORM: usize = 10;

impl Analyzer {
    /// 埋め込み辞書で初期化 (feature flag で IPADIC / UniDic 切替)
    ///
    /// # Errors
    /// 辞書ロードに失敗した場合 [`FuriganaError::AnalyzerInit`]。
    pub fn new() -> Result<Self> {
        let dictionary = load_dictionary(EMBEDDED_DICT_URI)
            .map_err(|e| FuriganaError::AnalyzerInit(format!("dictionary load: {e}")))?;
        let segmenter = Segmenter::new(Mode::Normal, dictionary, None);
        let tokenizer = Tokenizer::new(segmenter);
        Ok(Self {
            tokenizer,
            lattices: Mutex::new(Vec::new()),
        })
    }

    /// テキストを分解してトークン列を返す
    ///
    /// 形態素解析が失敗 / Mutex が poison された場合は、入力全体を 1 トークン
    /// (surface のみ) として返す — 呼び出し側が常に何らかの結果を扱える保証。
    #[must_use]
    pub fn tokenize(&self, text: &str) -> Vec<MorphToken> {
        self.tokenize_with(text, MorphToken::surface_only, |surface, details| {
            let get_detail = |i: usize| detail_at(details, i).map(ToString::to_string);
            MorphToken {
                surface: surface.to_string(),
                reading: reading_of(details),
                pos: details.first().map(ToString::to_string),
                pos_detail: get_detail(1),
                pos_detail2: get_detail(2),
                pos_detail3: get_detail(3),
                conjugation_type: get_detail(4),
                conjugation_form: get_detail(5),
                base_form: get_detail(FIELD_BASE_FORM),
            }
        })
    }

    /// IPADIC 辞書本体 (単語コスト / 連接コスト / 品詞) を借りて `f` を実行する。
    ///
    /// コスト lattice engine ([`crate::scoring::lattice`]) が単語列挙と連接コストに使う。
    /// 辞書は読み取り専用で共有しているので lock は要らない (他の解析と並列に動く)。
    pub(crate) fn with_dictionary<R>(&self, f: impl FnOnce(&Dictionary) -> R) -> R {
        f(&self.tokenizer.segmenter.dictionary)
    }

    /// lattice を 1 つ借りる (無ければ作る)。 poison は握り潰さず回復して続ける
    fn take_lattice(&self) -> Lattice {
        self.lattices
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop()
            .unwrap_or_default()
    }

    /// 借りた lattice を返す。 巨大入力の後は捨てる (容量が伸びたまま常駐しないように)
    fn return_lattice(&self, lattice: Lattice, text_len: usize) {
        if text_len > LATTICE_RETAIN_MAX_BYTES {
            return;
        }
        let mut pool = self
            .lattices
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if pool.len() < LATTICE_POOL_MAX {
            pool.push(lattice);
        }
    }

    /// crate 内部用の軽量 tokenize: surface / reading / 固有名詞判定だけを返す。
    ///
    /// [`Self::tokenize`] は 1 形態素ごとに品詞・活用・原形の `String` を 6 本確保するが、
    /// 解析 pipeline (Lindera fallback / 人名 post-pass / accent 推定) が見るのは
    /// 固有名詞・人名かどうかだけなので、 その 2 つを bool で持つ。
    /// 失敗時の振る舞い (入力全体を surface-only 1 token) は [`Self::tokenize`] と同じ。
    pub(crate) fn tokenize_light(&self, text: &str) -> Vec<LightMorph> {
        if text.is_empty() {
            return Vec::new();
        }
        let mut lattice = self.take_lattice();
        let result = match self.tokenizer.tokenize_with_lattice(text, &mut lattice) {
            Ok(mut tokens) => LIGHT_CACHE.with(|cache| {
                let mut cache = cache.borrow_mut();
                if cache.len() > LIGHT_CACHE_MAX {
                    cache.clear();
                }
                tokens
                    .iter_mut()
                    .map(|t| {
                        // 品詞などの派生値は語 id だけで決まるので、 id ごとに 1 度だけ details を分解する
                        // (details の分解は形態素ごとに Vec を何度も確保する)。
                        let key = u64::from(t.word_id.id) | (t.word_id.lex_type as u64) << 32;
                        let mut morph = if let Some(m) = cache.get(&key) {
                            m.clone()
                        } else {
                            let m = Self::light_from_details("", &t.details());
                            cache.insert(key, m.clone());
                            m
                        };
                        morph.surface = t.surface.to_string();
                        morph
                    })
                    .collect()
            }),
            Err(e) => {
                tracing::warn!("tokenize error: {e}");
                vec![LightMorph {
                    surface: text.to_string(),
                    reading: None,
                    is_proper_noun: false,
                    is_person_name: false,
                    is_ichidan_verb: false,
                    attaches_to_verb: false,
                    is_noun_suffix: false,
                    is_particle: false,
                    is_prefix: false,
                    is_pronoun: false,
                    is_noun: false,
                    is_place: false,
                }]
            }
        };
        self.return_lattice(lattice, text.len());
        result
    }

    /// details から [`LightMorph`] を作る (surface は呼び出し側が入れる)。
    fn light_from_details(surface: &str, details: &[&str]) -> LightMorph {
        let is_proper_noun =
            details.first() == Some(&"名詞") && detail_at(details, 1) == Some("固有名詞");
        LightMorph {
            surface: surface.to_string(),
            reading: reading_of(details),
            is_proper_noun,
            is_person_name: is_proper_noun && detail_at(details, 2) == Some("人名"),
            is_ichidan_verb: details.first() == Some(&"動詞")
                && detail_at(details, 1) == Some("自立")
                && detail_at(details, 4).is_some_and(|t| t.starts_with("一段")),
            attaches_to_verb: match (details.first().copied(), detail_at(details, 1)) {
                // 断定 (だ / です) は名詞にも付くので含めない
                (Some("助動詞"), _) => {
                    !detail_at(details, 4).is_some_and(|t| t == "特殊・ダ" || t == "特殊・デス")
                }
                (Some("助詞"), Some("接続助詞")) => true,
                // 動詞が続く形 = 複合動詞の前半 (見終わる / 出とる / 寝落ちる)。
                // 自立・非自立は問わない (西日本の 「とる」 は IPADIC では自立動詞)
                (Some("動詞"), _) => true,
                _ => false,
            },
            is_noun_suffix: details.first() == Some(&"名詞")
                && detail_at(details, 1) == Some("接尾"),
            is_particle: details.first() == Some(&"助詞"),
            is_prefix: details.first() == Some(&"接頭詞"),
            is_pronoun: details.first() == Some(&"名詞") && detail_at(details, 1) == Some("代名詞"),
            is_noun: details.first() == Some(&"名詞"),
            is_place: is_proper_noun && detail_at(details, 2) == Some("地域"),
        }
    }

    /// tokenize の共通土台。 lock 取得・lattice 使い回し・失敗時 fallback を持ち、
    /// 各形態素を `map(surface, details)` で変換する。
    fn tokenize_with<T>(
        &self,
        text: &str,
        fallback: impl FnOnce(&str) -> T,
        mut map: impl FnMut(&str, &[&str]) -> T,
    ) -> Vec<T> {
        if text.is_empty() {
            return Vec::new();
        }

        let mut lattice = self.take_lattice();

        let result = match self.tokenizer.tokenize_with_lattice(text, &mut lattice) {
            Ok(mut tokens) => tokens
                .iter_mut()
                .map(|t| {
                    // surface は入力の借用 (Cow::Borrowed) なので clone は参照コピーのみ
                    let surface = t.surface.clone();
                    let details = t.details();
                    map(&surface, &details)
                })
                .collect(),
            Err(e) => {
                tracing::warn!("tokenize error: {e}");
                vec![fallback(text)]
            }
        };
        self.return_lattice(lattice, text.len());
        result
    }
}

/// 語 id → 派生した [`LightMorph`] (surface 以外) の、 スレッドごとの cache。
/// 辞書は埋め込みの 1 つだけなので id は一意。 ロックを取らないよう thread_local にしてある。
const LIGHT_CACHE_MAX: usize = 200_000;
thread_local! {
    static LIGHT_CACHE: std::cell::RefCell<crate::fxmap::FxMap<u64, LightMorph>> =
        std::cell::RefCell::new(crate::fxmap::FxMap::default());
}

/// [`Analyzer::tokenize_light`] の 1 形態素。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LightMorph {
    pub surface: String,
    pub reading: Option<String>,
    /// 品詞 = 名詞 / 固有名詞
    pub is_proper_noun: bool,
    /// 品詞 = 名詞 / 固有名詞 / 人名
    pub is_person_name: bool,
    /// 品詞 = 動詞/自立 かつ 活用型 = 一段
    pub is_ichidan_verb: bool,
    /// 動詞に後接する語 (助動詞 / 接続助詞 / 非自立動詞)
    pub attaches_to_verb: bool,
    /// 品詞 = 名詞 / 接尾 (IPADIC は 疲れ=ヅカレ / 使い=ヅカイ 等の連濁形を接尾として持つ)
    pub is_noun_suffix: bool,
    /// 品詞 = 助詞
    pub is_particle: bool,
    /// 品詞 = 接頭詞 (お / ご)
    pub is_prefix: bool,
    /// 品詞 = 名詞 / 代名詞 (これ / それ / ここ)
    pub is_pronoun: bool,
    /// 品詞 = 名詞 (細分類は問わない)
    pub is_noun: bool,
    /// 品詞 = 名詞 / 固有名詞 / 地域
    pub is_place: bool,
}

/// details[i] を返す。 `*` と空文字は `None` に正規化する。
fn detail_at<'d>(details: &[&'d str], i: usize) -> Option<&'d str> {
    details
        .get(i)
        .copied()
        .filter(|v| *v != "*" && !v.is_empty())
}

/// details から読み (カタカナ) を取り出す。
fn reading_of(details: &[&str]) -> Option<String> {
    // ★alpha.17: UniDic は pron が長音符 「ー」 で長音を表すので
    // (例: 「学校=ガッコー」)、 表記読み (「ガッコウ」) に正規化する。
    // IPADIC では適用しない: 外来語 reading は正当に ー を含み
    // (カーテン 等)、 正規化すると カアテン に化ける (0.2.0 で修正、
    // 旧実装は無条件適用で IPADIC 外来語の長音が母音化けしていた)。
    #[cfg(all(feature = "dict-unidic", not(feature = "dict-ipadic")))]
    return detail_at(details, FIELD_READING).map(crate::kana::normalize_long_vowel);
    #[cfg(all(feature = "dict-ipadic", not(feature = "dict-unidic")))]
    return detail_at(details, FIELD_READING).map(ToString::to_string);
}

impl std::fmt::Debug for Analyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Analyzer").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyzer() -> Analyzer {
        Analyzer::new().expect("Analyzer init failed")
    }

    #[test]
    fn tokenizes_basic_japanese() {
        let a = analyzer();
        let tokens = a.tokenize("私は学生です");
        // surface 列を完全固定 (「私 が含まれる」 程度だと分割崩れを見逃す)。
        let surfaces: Vec<&str> = tokens.iter().map(|t| t.surface.as_str()).collect();
        assert_eq!(surfaces, vec!["私", "は", "学生", "です"]);
        // 名詞「学生」 の品詞と読みも値で固定する。
        let gakusei = tokens
            .iter()
            .find(|t| t.surface == "学生")
            .expect("学生 token");
        assert_eq!(gakusei.pos.as_deref(), Some("名詞"));
        assert_eq!(gakusei.reading.as_deref(), Some("ガクセイ"));
    }

    #[test]
    fn returns_reading_for_known_kanji() {
        let a = analyzer();
        let tokens = a.tokenize("読書");
        // 「どこかに reading が付く」 だと誤読でも緑になる。surface と reading を
        // 値で固定する (IPADIC は 「読書」 を 1 token ドクショ で返す)。
        let dokusho = tokens
            .iter()
            .find(|t| t.surface == "読書")
            .expect("読書 token");
        assert_eq!(dokusho.reading.as_deref(), Some("ドクショ"));
    }

    #[test]
    fn empty_input_yields_empty() {
        let a = analyzer();
        assert!(a.tokenize("").is_empty());
    }

    #[test]
    fn handles_mixed_script() {
        let a = analyzer();
        let tokens = a.tokenize("Hello世界123");
        // 「世界」が含まれる
        assert!(tokens.iter().any(|t| t.surface.contains("世")));
    }

    #[test]
    fn details_filter_asterisks_to_none() {
        // 助詞 (e.g., は) は activation_form 等が "*" になることが多い
        let a = analyzer();
        let tokens = a.tokenize("私は");
        // if let Some だと 「は」 が取れなければ無条件 pass していた。token 存在を強制。
        let token = tokens.iter().find(|t| t.surface == "は").expect("は token");
        // 助詞「は」は活用しないので conjugation_type は "*" → None 正規化されるはず
        assert!(token.conjugation_type.is_none());
    }

    /// `light_from_details` の各 flag を品詞ごとに値で固定する (演算子の取り違えを検出する)。
    #[test]
    fn light_from_details_flags_by_pos() {
        let f = |d: &[&str]| Analyzer::light_from_details("", d);
        // 助動詞: 断定 (だ / です) 以外は動詞に付く
        let da = f(&["助動詞", "*", "*", "*", "特殊・ダ", "基本形", "だ"]);
        assert!(!da.attaches_to_verb, "だ は名詞にも付くので含めない");
        let desu = f(&["助動詞", "*", "*", "*", "特殊・デス", "基本形", "です"]);
        assert!(!desu.attaches_to_verb, "です も含めない");
        let ta = f(&["助動詞", "*", "*", "*", "特殊・タ", "基本形", "た"]);
        assert!(ta.attaches_to_verb);
        let masu = f(&["助動詞", "*", "*", "*", "特殊・マス", "基本形", "ます"]);
        assert!(masu.attaches_to_verb);
        // 活用型なし (None) の助動詞は 「断定ではない」 = 付く
        let none = f(&["助動詞", "*", "*", "*", "*", "*", "*"]);
        assert!(none.attaches_to_verb);
        // 接続助詞は付く / 他の助詞は付かない
        let te = f(&["助詞", "接続助詞", "*", "*", "*", "*", "て"]);
        assert!(te.attaches_to_verb);
        assert!(te.is_particle);
        let ha = f(&["助詞", "係助詞", "*", "*", "*", "*", "は"]);
        assert!(!ha.attaches_to_verb);
        assert!(ha.is_particle);
        // 動詞は自立・非自立を問わず付く
        let v1 = f(&["動詞", "自立", "*", "*", "五段・ラ行", "基本形", "とる"]);
        assert!(v1.attaches_to_verb);
        let v2 = f(&["動詞", "非自立", "*", "*", "一段", "基本形", "いる"]);
        assert!(v2.attaches_to_verb);
        // 名詞などは付かない
        let n = f(&["名詞", "一般", "*", "*", "*", "*", "犬"]);
        assert!(!n.attaches_to_verb);
        assert!(n.is_noun);
        assert!(!n.is_particle);
        // 助動詞以外で活用型が特殊・ダ でも付く判定にならない / 助詞以外の接続助詞名
        let other = f(&["名詞", "接続助詞", "*", "*", "*", "*", "x"]);
        assert!(!other.attaches_to_verb);

        // 一段動詞: 動詞 + 自立 + 活用型が 一段 で始まる、 の全部が要る
        let ichi = f(&["動詞", "自立", "*", "*", "一段", "基本形", "見る"]);
        assert!(ichi.is_ichidan_verb);
        assert!(!f(&["動詞", "非自立", "*", "*", "一段", "基本形", "いる"]).is_ichidan_verb);
        assert!(!f(&["動詞", "自立", "*", "*", "五段・ラ行", "基本形", "取る"]).is_ichidan_verb);
        assert!(!f(&["名詞", "自立", "*", "*", "一段", "*", "x"]).is_ichidan_verb);
        assert!(!f(&["動詞", "自立", "*", "*", "*", "*", "x"]).is_ichidan_verb);

        // 固有名詞 / 人名 / 地域
        let person = f(&["名詞", "固有名詞", "人名", "姓", "*", "*", "田中"]);
        assert!(person.is_proper_noun && person.is_person_name && !person.is_place);
        let place = f(&["名詞", "固有名詞", "地域", "一般", "*", "*", "東京"]);
        assert!(place.is_proper_noun && place.is_place && !place.is_person_name);
        // 固有名詞でなければ細分類が 人名 / 地域 でも立たない
        let fake = f(&["名詞", "一般", "人名", "*", "*", "*", "x"]);
        assert!(!fake.is_person_name && !fake.is_place && !fake.is_proper_noun);
        let fake2 = f(&["動詞", "固有名詞", "地域", "*", "*", "*", "x"]);
        assert!(!fake2.is_proper_noun && !fake2.is_place);

        // 接尾 / 代名詞 / 接頭詞
        let suf = f(&["名詞", "接尾", "一般", "*", "*", "*", "づかれ"]);
        assert!(suf.is_noun_suffix && suf.is_noun && !suf.is_pronoun);
        let pro = f(&["名詞", "代名詞", "一般", "*", "*", "*", "これ"]);
        assert!(pro.is_pronoun && pro.is_noun && !pro.is_noun_suffix);
        let pre = f(&["接頭詞", "名詞接続", "*", "*", "*", "*", "お"]);
        assert!(pre.is_prefix && !pre.is_noun);
        assert!(!f(&["名詞", "接尾", "*", "*", "*", "*", "x"]).is_prefix);
    }

    /// 語 id キャッシュ: 同じ語を繰り返しても結果が変わらず、 キャッシュ key は語種 (lex_type) を含む。
    #[test]
    fn tokenize_light_cache_is_consistent_and_keyed_by_lex_type() {
        let a = analyzer();
        let text = "私は学生です。xyzqwvbn と ガギグゲゴ";
        LIGHT_CACHE.with(|c| c.borrow_mut().clear());
        let first = a.tokenize_light(text);
        let second = a.tokenize_light(text); // 2 回目は cache 経由
        assert_eq!(first, second);
        // cache を使わない経路 (tokenize の details 分解) と品詞 flag が一致する
        let heavy = a.tokenize(text);
        assert_eq!(first.len(), heavy.len());
        for (l, h) in first.iter().zip(&heavy) {
            assert_eq!(l.surface, h.surface);
            assert_eq!(
                l.is_particle,
                h.pos.as_deref() == Some("助詞"),
                "{}",
                l.surface
            );
            assert_eq!(l.is_noun, h.pos.as_deref() == Some("名詞"), "{}", l.surface);
        }
        // 未知語 (Unknown 語種) の key は上位 32bit に語種が入る。 system 語と id が重なっても別 entry になる
        let keys: Vec<u64> = LIGHT_CACHE.with(|c| c.borrow().keys().copied().collect());
        assert!(
            keys.iter().any(|k| k >> 32 != 0),
            "未知語の key に lex_type が入るはず: {keys:?}"
        );
        assert!(keys.iter().any(|k| k >> 32 == 0), "system 語の key");
        // 全 key について id 部分は 32bit に収まり、 語種部分は 0..=2
        assert!(keys.iter().all(|k| (k >> 32) <= 2));
    }

    /// キャッシュは LIGHT_CACHE_MAX を超えた時だけ空にする (ちょうど MAX では空にしない)。
    #[test]
    fn tokenize_light_cache_clears_only_above_max() {
        let a = analyzer();
        let dummy = Analyzer::light_from_details("", &["名詞"]);
        let fill = |n: usize| {
            LIGHT_CACHE.with(|c| {
                let mut c = c.borrow_mut();
                c.clear();
                for i in 0..n {
                    // 実在の語 id (上位 32bit が 0..=2) と衝突しない key
                    c.insert(u64::MAX - i as u64, dummy.clone());
                }
            });
        };
        // ちょうど MAX: 空にしない
        fill(LIGHT_CACHE_MAX);
        a.tokenize_light("学生");
        let len = LIGHT_CACHE.with(|c| c.borrow().len());
        assert!(
            len >= LIGHT_CACHE_MAX,
            "MAX ちょうどでは clear しない: {len}"
        );
        // MAX 超: 空にしてから入れ直す
        fill(LIGHT_CACHE_MAX + 1);
        a.tokenize_light("学生");
        let len = LIGHT_CACHE.with(|c| c.borrow().len());
        assert!(len < 100, "MAX 超では clear する: {len}");
        // 後始末
        LIGHT_CACHE.with(|c| c.borrow_mut().clear());
    }

    #[test]
    fn tokenize_recovers_from_poisoned_mutex() {
        use std::sync::Arc;
        let a = Arc::new(analyzer());
        // 別 thread で tokenizer lock 保持中に panic させ mutex を poison させる。
        let a2 = Arc::clone(&a);
        let handle = std::thread::spawn(move || {
            let _guard = a2.lattices.lock().unwrap();
            panic!("intentional panic while holding lattice pool lock");
        });
        assert!(handle.join().is_err(), "panic で poison したはず");
        // poison 後も tokenize は recover して継続する (恒久 degrade に陥らない)。
        // 旧実装は surface-only fallback (= 1 token) を返していた。
        let tokens = a.tokenize("私は学生です");
        assert!(
            tokens.len() > 1,
            "poison から回復して形態素解析が継続するはず (surface-only ではない): {tokens:?}"
        );
    }

    #[test]
    fn analyzer_is_shareable_across_threads() {
        fn assert_sync<T: Send + Sync>() {}
        assert_sync::<Analyzer>();
    }

    /// 8 スレッドが同時に解析しても、 1 スレッドで解析した結果と全部一致する (共有 tokenizer + lattice プール)
    #[test]
    fn concurrent_tokenize_matches_sequential() {
        use std::sync::Arc;
        let a = Arc::new(analyzer());
        let inputs = [
            "私は学生です",
            "今日は雨が降っています",
            "東京特許許可局",
            "ありがとうございました",
            "猫が好きな人",
        ];
        let expected: Vec<Vec<MorphToken>> = inputs.iter().map(|t| a.tokenize(t)).collect();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let a = Arc::clone(&a);
                let expected = expected.clone();
                std::thread::spawn(move || {
                    for round in 0..200 {
                        let k = (i + round) % inputs.len();
                        assert_eq!(a.tokenize(inputs[k]), expected[k]);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("thread panicked");
        }
    }
}
