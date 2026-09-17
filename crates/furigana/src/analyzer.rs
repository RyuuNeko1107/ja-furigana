//! 形態素解析器 (Lindera + IPADIC ラッパ)
//!
//! [`Analyzer::new`] で IPADIC を埋め込みロードし、`tokenize` で
//! テキストを [`MorphToken`] 列に分解する。
//!
//! Lindera 自体の `Tokenizer` はスレッドセーフではないため、
//! 内部で `Mutex` 保護している。複数スレッドから同時呼び出し可能だが
//! 直列実行になることに注意。

use crate::error::{FuriganaError, Result};
use lindera::dictionary::load_dictionary;
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
    /// カタカナ読み (IPADIC details[7])
    pub reading: Option<String>,
    /// 品詞 (IPADIC details[0]) — 名詞 / 動詞 / 形容詞 等
    pub pos: Option<String>,
    /// 品詞細分類 1 (IPADIC details[1]) — 普通名詞 / 固有名詞 / 数 等
    pub pos_detail: Option<String>,
    /// 品詞細分類 2 (IPADIC details[2]) — 人名 / 地域 / 組織 等 (固有名詞の下位分類)
    pub pos_detail2: Option<String>,
    /// 活用型 (IPADIC details[4]) — 五段・カ行イ音便 / 一段 等
    pub conjugation_type: Option<String>,
    /// 活用形 (IPADIC details[5]) — 基本形 / 連用形 等
    pub conjugation_form: Option<String>,
    /// 原形 (IPADIC details[6]) — 「食べた」→「食べる」
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
            conjugation_type: None,
            conjugation_form: None,
            base_form: None,
        }
    }
}

/// 形態素解析器
pub struct Analyzer {
    tokenizer: Mutex<TokenizerState>,
}

/// Mutex で守る tokenizer 本体 + 使い回す lattice。
///
/// `Tokenizer::tokenize` は呼ぶたびに `Lattice` を新規確保する (短文 1 回で約 14 KiB、
/// 解析 1 回の確保量の最大項)。 tokenizer はもともと Mutex で直列化しているので、
/// lattice も同じ lock の下に置いて `tokenize_with_lattice` で使い回す。
struct TokenizerState {
    tokenizer: Tokenizer,
    lattice: Lattice,
}

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
/// - IPADIC: details[7] (カタカナ reading)
/// - UniDic: details[9] (pron = 発音形出現形)
#[cfg(all(feature = "dict-ipadic", not(feature = "dict-unidic")))]
const FIELD_READING: usize = 7;
#[cfg(all(feature = "dict-unidic", not(feature = "dict-ipadic")))]
const FIELD_READING: usize = 9;

/// details field 番号: base_form (= 原形 / 辞書形)。
///
/// - IPADIC: details[6] (原形)
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
            tokenizer: Mutex::new(TokenizerState {
                tokenizer,
                lattice: Lattice::default(),
            }),
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
                conjugation_type: get_detail(4),
                conjugation_form: get_detail(5),
                base_form: get_detail(FIELD_BASE_FORM),
            }
        })
    }

    /// crate 内部用の軽量 tokenize: surface / reading / 固有名詞判定だけを返す。
    ///
    /// [`Self::tokenize`] は 1 形態素ごとに品詞・活用・原形の `String` を 6 本確保するが、
    /// 解析 pipeline (Lindera fallback / 人名 post-pass / accent 推定) が見るのは
    /// 固有名詞・人名かどうかだけなので、 その 2 つを bool で持つ。
    /// 失敗時の振る舞い (入力全体を surface-only 1 token) は [`Self::tokenize`] と同じ。
    pub(crate) fn tokenize_light(&self, text: &str) -> Vec<LightMorph> {
        self.tokenize_with(
            text,
            |t| LightMorph {
                surface: t.to_string(),
                reading: None,
                is_proper_noun: false,
                is_person_name: false,
                is_ichidan_verb: false,
                attaches_to_verb: false,
            },
            |surface, details| {
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
                        (Some("助動詞"), _) => !detail_at(details, 4)
                            .is_some_and(|t| t == "特殊・ダ" || t == "特殊・デス"),
                        (Some("助詞"), Some("接続助詞")) => true,
                        (Some("動詞"), Some("非自立")) => true,
                        _ => false,
                    },
                }
            },
        )
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

        // poison 時は lock を握り潰して surface-only に倒すと、 以降全 request が
        // 恒久 degrade (形態素解析なし) に固定されてしまう。 poison は別 thread の
        // panic 由来で、 `Tokenizer::tokenize` は `&self` (read-only、 内部可変は
        // Mutex で排他) なので、 lock を奪い返して継続するのが安全 (恒久 degrade 回避)。
        let mut guard = self.tokenizer.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("Tokenizer mutex poisoned; recovering lock and continuing");
            poisoned.into_inner()
        });
        let state = &mut *guard;

        let result = match state
            .tokenizer
            .tokenize_with_lattice(text, &mut state.lattice)
        {
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
        if text.len() > LATTICE_RETAIN_MAX_BYTES {
            state.lattice = Lattice::default();
        }
        result
    }
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

    #[test]
    fn tokenize_recovers_from_poisoned_mutex() {
        use std::sync::Arc;
        let a = Arc::new(analyzer());
        // 別 thread で tokenizer lock 保持中に panic させ mutex を poison させる。
        let a2 = Arc::clone(&a);
        let handle = std::thread::spawn(move || {
            let _guard = a2.tokenizer.lock().unwrap();
            panic!("intentional panic while holding tokenizer lock");
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
}
