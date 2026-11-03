//! What the text tests share: the fonts, which aren't committed yet, and a menu's worth of strings.
//!
//! Set `SAPPHIRE_TEST_FONTS` to a folder with `mplus1/MPLUS1[wght].ttf`,
//! `oxanium/Oxanium[wght].ttf` and `dela_gothic_one/DelaGothicOne-Regular.ttf`. Without it, a test
//! that needs them says so loudly and passes.

#![allow(dead_code, reason = "each test binary uses its own share of this")]

use std::path::PathBuf;
use std::sync::Arc;

use sapphire_text::{TextStyle, TextSystem};

pub const FAMILY_BODY: &str = "'M PLUS 1'";
pub const FAMILY_DISPLAY_LATIN: &str = "Oxanium, 'M PLUS 1'";
pub const FAMILY_DISPLAY_JA: &str = "'Dela Gothic One', 'M PLUS 1'";

/// The fonts' folder, or `None` after saying why this test isn't really running.
pub fn fonts_dir(test: &str) -> Option<PathBuf> {
    match std::env::var_os("SAPPHIRE_TEST_FONTS") {
        Some(dir) => Some(PathBuf::from(dir)),
        None => {
            eprintln!("*** {test} SKIPPED: SAPPHIRE_TEST_FONTS isn't set, so nothing was checked ***");
            None
        }
    }
}

pub fn read_font(dir: &std::path::Path, relative: &str) -> Arc<Vec<u8>> {
    let path = dir.join(relative);
    Arc::new(std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display())))
}

pub const MPLUS1: &str = "mplus1/MPLUS1[wght].ttf";
pub const OXANIUM: &str = "oxanium/Oxanium[wght].ttf";
pub const DELA: &str = "dela_gothic_one/DelaGothicOne-Regular.ttf";

/// A system with all three fonts.
pub fn system(dir: &std::path::Path) -> TextSystem {
    let mut text = TextSystem::new();
    for font in [MPLUS1, OXANIUM, DELA] {
        text.register_font(read_font(dir, font));
    }
    text
}

/// A style in the menu's type scale.
pub const fn style(families: &'static str, size: f32, weight: f32, locale: &'static str) -> TextStyle {
    TextStyle { families, size, weight, line_height: 1.3, locale: Some(locale) }
}

/// One string on a menu screen, placed by a stand-in for `taffy`.
#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub text: &'static str,
    pub style: TextStyle,
    pub max_width: Option<f32>,
}

const JA: &str = "ja";
const EN: &str = "en";

const SETTINGS_JA: [&str; 24] = [
    "画面の明るさ",
    "音楽の音量",
    "効果音の音量",
    "ボイスの音量",
    "ボイスの言語",
    "字幕を表示する",
    "カメラの感度",
    "カメラの上下反転",
    "振動の強さ",
    "攻撃ボタンの配置",
    "画質の設定",
    "フレームレート",
    "影の品質",
    "輪郭線の太さ",
    "画面の揺れを抑える",
    "文字の大きさ",
    "色覚サポート",
    "自動で戦闘する",
    "通信の状態を確認",
    "データをダウンロード",
    "キャッシュを削除",
    "お問い合わせ",
    "利用規約",
    "タイトルに戻る",
];

const SETTINGS_EN: [&str; 24] = [
    "Screen brightness",
    "Music volume",
    "Sound effects volume",
    "Voice volume",
    "Voice language",
    "Show subtitles",
    "Camera sensitivity",
    "Invert camera vertically",
    "Vibration strength",
    "Attack button position",
    "Graphics quality",
    "Frame rate",
    "Shadow quality",
    "Outline thickness",
    "Reduce screen shake",
    "Text size",
    "Color vision support",
    "Auto battle",
    "Check connection status",
    "Download data",
    "Clear cache",
    "Contact us",
    "Terms of use",
    "Return to title",
];

const STATS_JA: [&str; 16] = [
    "攻撃力",
    "防御力",
    "会心率",
    "会心ダメージ",
    "元素熟知",
    "最大体力",
    "移動速度",
    "回避性能",
    "通常攻撃",
    "重撃",
    "戦闘スキル",
    "必殺技",
    "連携スキル",
    "固有能力",
    "レベルアップ",
    "強化素材",
];

const STATS_EN: [&str; 16] = [
    "Attack",
    "Defense",
    "Crit rate",
    "Crit damage",
    "Elemental mastery",
    "Max health",
    "Move speed",
    "Evasion",
    "Basic attack",
    "Heavy attack",
    "Battle skill",
    "Ultimate",
    "Link skill",
    "Passive talent",
    "Level up",
    "Upgrade materials",
];

const DIALOGUE_JA: [&str; 5] = [
    "嵐が過ぎた朝、港の鐘がふたつ鳴った。「まだ船は戻らないのか」と老人は海を見つめたまま尋ねた。",
    "彼女は刀を鞘に収め、静かに首を振る。潮の匂いに混じって、遠くの森が焼ける煙の気配がした。",
    "「約束は守ります。夜明けまでに北の門を開けて、街の人たちを逃がしてください。」",
    "地図によれば、谷を抜ける道は三本あるが、橋が残っているのは一本だけだ。敵の見張りを避けるには、月が沈むのを待つしかない。",
    "Lv.90 の守護者を倒すと、限定装備「蒼の残光」とスキルポイントを 1,200 獲得できます。",
];

const DIALOGUE_EN: [&str; 5] = [
    "On the morning after the storm, the harbor bell rang twice. \"Has the ship still not come back?\" the old man asked, his eyes on the sea.",
    "She sheathed her sword and quietly shook her head. Under the smell of the tide came a trace of smoke from the burning forest far away.",
    "\"I'll keep my promise. Open the north gate before dawn and get the townspeople out.\"",
    "According to the map, three roads run through the valley, but only one still has its bridge. To avoid the enemy's lookouts we must wait for the moon to set.",
    "Defeat the Lv.90 guardian to earn the limited gear \"Azure Afterglow\" and 1,200 skill points.",
];

/// A settings, stats and dialogue screen, as dense as a real one, in the body font in several
/// weights, a display title, and a digit face.
pub fn menu_screen(japanese: bool) -> Vec<Item> {
    let (settings, stats, dialogue, locale, title, title_family, digits) = if japanese {
        (&SETTINGS_JA, &STATS_JA, &DIALOGUE_JA, JA, "設定と記録", FAMILY_DISPLAY_JA, "12,480")
    } else {
        (&SETTINGS_EN, &STATS_EN, &DIALOGUE_EN, EN, "SETTINGS", FAMILY_DISPLAY_LATIN, "12,480")
    };
    let mut items = vec![
        Item { text: title, style: style(title_family, 32.0, 800.0, locale), max_width: None },
        Item { text: digits, style: style(FAMILY_DISPLAY_LATIN, 20.0, 600.0, locale), max_width: None },
    ];
    let label = style(FAMILY_BODY, 14.0, 400.0, locale);
    let value = style(FAMILY_BODY, 16.0, 500.0, locale);
    let heading = style(FAMILY_BODY, 20.0, 700.0, locale);
    let caption = style(FAMILY_BODY, 12.0, 400.0, locale);
    for text in settings {
        items.push(Item { text, style: label, max_width: Some(360.0) });
    }
    for text in stats {
        items.push(Item { text, style: value, max_width: Some(240.0) });
    }
    for text in dialogue {
        items.push(Item { text, style: label, max_width: Some(600.0) });
    }
    items.push(Item { text: stats[0], style: heading, max_width: None });
    items.push(Item { text: settings[0], style: heading, max_width: None });
    items.push(Item { text: dialogue[4], style: caption, max_width: Some(600.0) });
    items
}

/// `count` different Japanese characters `font` has: kana first, then Han in code point order. No
/// real screen is this hard: every cell is a glyph the atlas hasn't seen.
pub fn distinct_japanese(font: &[u8], count: usize) -> String {
    let font = swash::FontRef::from_index(font, 0).expect("a font");
    let charmap = font.charmap();
    let kana = ('\u{3041}'..='\u{3093}').chain('\u{30A1}'..='\u{30F3}');
    kana.chain('\u{4E00}'..='\u{9FA0}').filter(|character| charmap.map(*character) != 0).take(count).collect()
}

/// A digest that doesn't depend on the OS or on hash order: FNV-1a over 64 bits.
#[derive(Clone, Copy, Debug)]
pub struct Digest(pub u64);

impl Default for Digest {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Digest {
    pub fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    pub fn u32(&mut self, value: u32) {
        self.bytes(&value.to_le_bytes());
    }

    pub fn f32(&mut self, value: f32) {
        self.u32(value.to_bits());
    }
}

/// The p-th percentile of `samples`.
pub fn percentile(samples: &mut [f64], p: f64) -> f64 {
    samples.sort_by(f64::total_cmp);
    let at = ((samples.len() - 1) as f64 * p).round() as usize;
    samples[at]
}
