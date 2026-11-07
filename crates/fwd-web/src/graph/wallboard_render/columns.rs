//! How wide the renderer reckons text, in columns: a character takes the columns Unicode's East Asian
//! Width gives it, one for most and two for a wide or fullwidth one, which fonts draw about a full em
//! wide. The renderer cannot measure the fonts the display draws with, so it wraps and shortens text
//! by this reckoning, and each card clips whatever still runs past its edges (`super::markup`).
use unicode_width::UnicodeWidthChar;

/// How many columns `c` takes: two for a character Unicode gives the East Asian Width Wide or
/// Fullwidth, such as CJK and most emoji, none for one drawn with no width of its own, such as a
/// combining mark, and one for any other.
pub(super) fn of(c: char) -> usize {
    match c as u32 {
        // Miscellaneous Symbols and Pictographs: Unicode gives some of them, such as 🌡, the width
        // Neutral, but headless Chromium draws them as pictures two columns wide, as it does the rest.
        0x1F300..=0x1F5FF => 2,
        // The emoji presentation selector makes the symbol before it, such as the ⚠ of ⚠️, a picture two
        // columns wide, though Unicode gives the symbol one.
        0xFE0F => 1,
        // A control character has no width; one is the safe count, since too few columns lets text run
        // into the card's clip, and too many only wraps it sooner.
        _ => c.width().unwrap_or(1),
    }
}

/// How many columns `text` takes.
pub(super) fn width(text: &str) -> usize {
    text.chars().map(of).sum()
}

/// `text` shortened to `columns` columns, with an ellipsis after it when shortened.
pub(super) fn truncate(text: &str, columns: usize) -> String {
    if width(text) <= columns {
        return text.to_string();
    }
    let mut taken = 0;
    let mut shortened: String = text
        .chars()
        .take_while(|&c| {
            taken += of(c);
            taken <= columns
        })
        .collect();
    shortened.push('…');
    shortened
}

/// How many of `chars`, from the first, fit in `columns` columns.
pub(super) fn fitting(chars: &[char], columns: usize) -> usize {
    let mut taken = 0;
    chars
        .iter()
        .take_while(|&&c| {
            taken += of(c);
            taken <= columns
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_character_takes_two_columns_and_any_other_one() {
        assert_eq!(width("Plan the release"), 16);
        assert_eq!(width("日本語のタスク"), 14, "kanji and kana");
        assert_eq!(width("한국어"), 6, "Hangul");
        assert_eq!(width("ＡＢＣ"), 6, "fullwidth Latin");
        assert_eq!(width("Ship 日本"), 9, "mixed");
        assert_eq!(width("ÉCOLE"), 5, "accented capitals are not wide");
        assert_eq!(width("E\u{301}COLE"), 5, "a combining accent takes no column of its own");
    }

    #[test]
    fn emoji_take_two_columns_each() {
        for (emoji, block) in [
            ("⌚", "Miscellaneous Technical"),
            ("✅", "Dingbats"),
            ("🚀🛸", "Transport and Map Symbols"),
            ("🩰🪄🩷🫶", "Symbols and Pictographs Extended-A"),
            ("😀🙏", "Emoticons"),
            ("🌡🗺", "Miscellaneous Symbols and Pictographs, Neutral in Unicode and drawn as pictures"),
        ] {
            for c in emoji.chars() {
                assert_eq!(of(c), 2, "{c} of {block}");
            }
        }
        assert_eq!(width("⚠️"), 2, "a one-column symbol the presentation selector draws as a picture");
    }

    #[test]
    fn shortening_counts_columns_and_says_where_it_shortened() {
        assert_eq!(truncate("Plan the release", 16), "Plan the release", "it fits");
        assert_eq!(truncate("Plan the release", 8), "Plan the…");
        assert_eq!(truncate("日本語のタスク", 7), "日本語…", "three wide characters, six columns, fit in seven");
        assert_eq!(fitting(&"日本語".chars().collect::<Vec<_>>(), 5), 2);
        assert_eq!(fitting(&"abc".chars().collect::<Vec<_>>(), 5), 3);
    }
}
