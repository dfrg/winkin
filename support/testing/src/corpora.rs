//! Text corpora of about 1 MB each, for timing segmentation.
//!
//! Each corpus repeats a short seed text of its script until it reaches
//! [`CORPUS_BYTES`], so the corpora are built in memory and nothing large is
//! checked in. The seeds are ordinary prose, with the punctuation, digits
//! and mixed scripts a segmenter meets in running text.

use alloc::string::String;
use alloc::vec::Vec;

/// The least size of each corpus, in UTF-8 bytes.
pub const CORPUS_BYTES: usize = 1 << 20;

/// A named corpus.
pub struct Corpus {
    /// A short name for reports.
    pub name: &'static str,
    /// The text, at least [`CORPUS_BYTES`] long.
    pub text: String,
}

const LATIN: &str = "The quick brown fox jumps over the lazy dog. Text layout \
    engines find line breaks, words and grapheme clusters; prices like $12.50 or \
    3.14159, dates such as 2026-10-22, and e-mail addresses (someone@example.com) \
    appear in running text too. \"Quoted words,\" said the editor, \"aren't rare.\"\n";

const JAPANESE: &str = "日本語の文章は、単語の間に空白を置かずに書かれます。\
    漢字とひらがな、カタカナを組み合わせて表記し、句読点の「、」と「。」で区切ります。\
    今日は天気が良いので、公園へ散歩に行きました。ソフトウェアの国際化には、\
    文字の境界を正しく見つけることが欠かせません。\n";

const CHINESE: &str = "中文书写时词与词之间没有空格。汉字、标点符号和数字123常常\
    混排在一起。今天天气很好，我们去公园散步，然后在图书馆读了两个小时的书。\
    软件的国际化需要正确地找到字符、词语和行的边界。\n";

const THAI: &str = "ภาษาไทยเป็นภาษาราชการของประเทศไทย มีผู้พูดเป็นภาษาแม่ประมาณหกสิบล้านคน \
    การเขียนภาษาไทยไม่เว้นวรรคระหว่างคำ แต่เว้นวรรคระหว่างวลีหรือประโยค \
    วันนี้อากาศดีมาก เราจึงไปเดินเล่นที่สวนสาธารณะใกล้บ้าน \
    โปรแกรมคอมพิวเตอร์ต้องใช้พจนานุกรมเพื่อหาขอบเขตของคำ\n";

const LAO: &str = "ພາສາລາວເປັນພາສາທາງການຂອງສາທາລະນະລັດປະຊາທິປະໄຕປະຊາຊົນລາວ \
    ມີຜູ້ເວົ້າປະມານສາມລ້ານຄົນ ການຂຽນພາສາລາວບໍ່ມີການເວັ້ນວັກລະຫວ່າງຄໍາ \
    ມື້ນີ້ອາກາດດີຫຼາຍ ພວກເຮົາຈຶ່ງໄປຍ່າງຫຼິ້ນຢູ່ສວນສາທາລະນະໃກ້ເຮືອນ \
    ໂປຣແກຣມຄອມພິວເຕີຕ້ອງໃຊ້ວັດຈະນານຸກົມເພື່ອຊອກຫາຂອບເຂດຂອງຄໍາ\n";

const KHMER: &str = "ភាសាខ្មែរគឺជាភាសាផ្លូវការរបស់ប្រទេសកម្ពុជា \
    ដែលមានអ្នកនិយាយប្រហែលដប់ប្រាំលាននាក់ ការសរសេរភាសាខ្មែរមិនដកឃ្លារវាងពាក្យទេ \
    ថ្ងៃនេះអាកាសធាតុល្អណាស់ យើងទៅដើរលេងនៅសួនច្បារសាធារណៈក្បែរផ្ទះ \
    កម្មវិធីកុំព្យូទ័រត្រូវប្រើវចនានុក្រមដើម្បីរកព្រំដែននៃពាក្យ\n";

const MYANMAR: &str = "မြန်မာဘာသာစကားသည် မြန်မာနိုင်ငံ၏ရုံးသုံးဘာသာစကားဖြစ်သည်။ \
    စကားလုံးများကြားတွင် ကွက်လပ်မထားဘဲ ရေးသားလေ့ရှိသည်။ \
    ယနေ့ရာသီဥတုအလွန်ကောင်းသဖြင့် ကျွန်ုပ်တို့သည် အိမ်အနီးရှိပန်းခြံသို့ လမ်းလျှောက်သွားကြသည်။ \
    ကွန်ပျူတာပရိုဂရမ်များသည် စကားလုံးနယ်နိမိတ်ကိုရှာရန် အဘိဓာန်ကိုအသုံးပြုရသည်။\n";

const MIXED: &str = "Hello 世界! Привет, мир. مرحبا بالعالم. नमस्ते दुनिया। \
    안녕하세요 세계. ภาษาไทยง่ายนิดเดียว Γειά σου κόσμε. שלום עולם. \
    👍🏽 a family 👨‍👩‍👧 and flags 🇯🇵🇫🇷, café, naïve, ﬁne, x² ≤ 10.\n";

/// Returns `seed` repeated until it is at least [`CORPUS_BYTES`] long.
fn repeated(seed: &str) -> String {
    let mut text = String::with_capacity(CORPUS_BYTES + seed.len());
    while text.len() < CORPUS_BYTES {
        text.push_str(seed);
    }
    text
}

/// Returns the segmentation corpora: Latin, Japanese, Chinese, Thai with
/// spaces between phrases, one Thai run with no space or line end, Lao,
/// Khmer and Myanmar with spaces between phrases, and text that mixes
/// scripts and emoji.
pub fn segmentation_corpora() -> Vec<Corpus> {
    let unspaced: String = THAI.chars().filter(|ch| !ch.is_whitespace()).collect();
    Vec::from([
        Corpus {
            name: "Latin",
            text: repeated(LATIN),
        },
        Corpus {
            name: "Japanese",
            text: repeated(JAPANESE),
        },
        Corpus {
            name: "Chinese",
            text: repeated(CHINESE),
        },
        Corpus {
            name: "Thai, spaced",
            text: repeated(THAI),
        },
        Corpus {
            name: "Thai, one run",
            text: repeated(&unspaced),
        },
        Corpus {
            name: "Lao",
            text: repeated(LAO),
        },
        Corpus {
            name: "Khmer",
            text: repeated(KHMER),
        },
        Corpus {
            name: "Myanmar",
            text: repeated(MYANMAR),
        },
        Corpus {
            name: "mixed",
            text: repeated(MIXED),
        },
    ])
}
