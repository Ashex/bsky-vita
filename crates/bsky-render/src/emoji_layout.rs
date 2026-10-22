use unicode_segmentation::UnicodeSegmentation;

/// Remove text-presentation selectors before sending text to the Vita font
/// backend, which may otherwise attempt to render the selector as a glyph.
pub fn normalize_text(input: &str) -> std::borrow::Cow<'_, str> {
    if input.contains('\u{fe0e}') {
        std::borrow::Cow::Owned(input.replace('\u{fe0e}', ""))
    } else {
        std::borrow::Cow::Borrowed(input)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token<'a> {
    Text(&'a str),
    Emoji(&'a str),
    Fallback(&'a str),
}

fn is_emoji_cluster(cluster: &str) -> bool {
    if cluster.contains('\u{fe0e}') {
        return false;
    }
    // Symbols with both text and emoji presentation require VS16 to be
    // treated as emoji; their bare forms (and otherwise ordinary symbols)
    // stay in the font text run.
    let emoji_capable_symbol = |cp: u32| matches!(cp,
        0x00a9 | 0x00ae | 0x203c | 0x2049 | 0x2122 | 0x2139 | 0x2300..=0x23ff | 0x3030 | 0x303d | 0x3297 | 0x3299
    );
    if emoji_capable_symbol(cluster.chars().next().unwrap_or('\0') as u32) {
        return cluster.contains('\u{fe0f}');
    }
    cluster.chars().any(|ch| {
        let cp = ch as u32;
        (0x1f000..=0x1faff).contains(&cp)
            || (0x2600..=0x27ff).contains(&cp)
            || (0x2b00..=0x2bff).contains(&cp)
            || matches!(cp, 0x1f1e6..=0x1f1ff | 0x1f3f4)
    })
}

pub fn tokens(input: &str, mut supported: impl FnMut(&str) -> bool) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut text_start = 0;
    for (start, grapheme) in input.grapheme_indices(true) {
        let is_emoji = is_emoji_cluster(grapheme);
        if supported(grapheme) {
            if text_start < start {
                out.push(Token::Text(&input[text_start..start]));
            }
            out.push(Token::Emoji(grapheme));
            text_start = start + grapheme.len();
        } else if is_emoji {
            if text_start < start {
                out.push(Token::Text(&input[text_start..start]));
            }
            out.push(Token::Fallback(grapheme));
            text_start = start + grapheme.len();
        }
    }
    if text_start < input.len() {
        out.push(Token::Text(&input[text_start..]));
    }
    out
}

pub fn layout(
    input: &str,
    max_width: i32,
    space_width: i32,
    mut width: impl FnMut(&str) -> i32,
) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in input.split('\n') {
        let mut line = String::new();
        let mut line_width: i32 = 0;
        for word in paragraph.split_whitespace() {
            let word_width = width(word);
            let separator = i32::from(!line.is_empty()) * space_width;
            if !line.is_empty()
                && line_width
                    .saturating_add(separator)
                    .saturating_add(word_width)
                    > max_width
            {
                lines.push(std::mem::take(&mut line));
                line_width = 0;
            }
            if word_width > max_width && max_width >= 0 {
                for grapheme in word.graphemes(true) {
                    let grapheme_width = width(grapheme);
                    if !line.is_empty() && line_width.saturating_add(grapheme_width) > max_width {
                        lines.push(std::mem::take(&mut line));
                        line_width = 0;
                    }
                    line.push_str(grapheme);
                    line_width = line_width.saturating_add(grapheme_width);
                }
            } else {
                if !line.is_empty() {
                    line.push(' ');
                    line_width = line_width.saturating_add(space_width);
                }
                line.push_str(word);
                line_width = line_width.saturating_add(word_width);
            }
        }
        if !line.is_empty() {
            lines.push(line);
        } else if paragraph.is_empty() && !input.is_empty() {
            lines.push(String::new());
        }
    }
    lines
}

pub fn truncated_prefix_end(input: &str, mut fits: impl FnMut(&str) -> bool) -> usize {
    let mut end = 0;
    for (start, grapheme) in input.grapheme_indices(true) {
        let next = start + grapheme.len();
        if !fits(&input[..next]) {
            break;
        }
        end = next;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_table_recognizes_complete_supported_sequences() {
        for sequence in ["❤️", "1️⃣", "👍🏽", "🇬🇧", "👩‍💻", "🫶"] {
            let result = tokens(sequence, |cluster| super::super::emoji_table::lookup_sequence(cluster).is_some());
            assert_eq!(result, vec![Token::Emoji(sequence)], "{sequence}");
        }
    }

    #[test]
    fn generated_table_preserves_text_presentation_and_falls_back_once() {
        assert_eq!(tokens("❤︎", |cluster| super::super::emoji_table::lookup_sequence(cluster).is_some()), vec![Token::Text("❤︎")]);
        for sequence in ["👨‍👩‍👧‍👦", "🏴", "🫱🏻‍🫲🏿"] {
            assert_eq!(tokens(sequence, |cluster| super::super::emoji_table::lookup_sequence(cluster).is_some()), vec![Token::Fallback(sequence)], "{sequence}");
        }
    }

    #[test]
    fn tokenizer_preserves_clusters_and_text() {
        let s = "x👍🏽🇬🇧1️⃣👩‍💻🫶y";
        let result = tokens(s, |g| ["👍🏽", "🇬🇧", "1️⃣", "👩‍💻", "🫶"].contains(&g));
        assert_eq!(
            result,
            vec![
                Token::Text("x"),
                Token::Emoji("👍🏽"),
                Token::Emoji("🇬🇧"),
                Token::Emoji("1️⃣"),
                Token::Emoji("👩‍💻"),
                Token::Emoji("🫶"),
                Token::Text("y")
            ]
        );
    }

    #[test]
    fn ordinary_symbols_stay_text_and_emoji_presentation_is_classified() {
        for symbol in ["⌘", "⌥", "©", "™"] {
            assert_eq!(tokens(symbol, |_| false), vec![Token::Text(symbol)]);
        }
        assert_eq!(tokens("©️", |_| false), vec![Token::Fallback("©️")]);
        assert_eq!(tokens("⌘️", |_| false), vec![Token::Fallback("⌘️")]);
    }

    #[test]
    fn text_selector_is_removed_from_text_normalization_and_not_emoji() {
        assert_eq!(normalize_text("❤︎"), "❤");
        assert_eq!(tokens("❤︎", |_| false), vec![Token::Text("❤︎")]);
    }

    #[test]
    fn layout_wraps_long_words_and_preserves_blank_rows() {
        let lines = layout("abcdef\n\nxy", 2, 1, |s| s.graphemes(true).count() as i32);
        assert_eq!(lines, vec!["ab", "cd", "ef", "", "xy"]);
    }

    #[test]
    fn truncation_ends_on_cluster_boundary() {
        let s = "A👩‍💻Z";
        let end = truncated_prefix_end(s, |prefix| prefix.len() <= "A👩‍💻".len());
        assert_eq!(&s[..end], "A👩‍💻");
    }
}
