//! Visible escapes for layout controls in exact tool data, never its execution value.
use std::fmt::Write;
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

pub fn escape(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        // A carriage return that is part of a line ending is not content.
        if character == '\r' && characters.peek() == Some(&'\n') {
            continue;
        }
        if matches!(
            character.general_category(),
            GeneralCategory::Format
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
        ) || (character.is_control() && character != '\n' && character != '\t')
        {
            for unit in character.encode_utf16(&mut [0; 2]) {
                write!(result, "\\u{unit:04x}").expect("writing a string");
            }
        } else {
            result.push(character);
        }
    }
    result
}

pub fn deserialize<'de, D: serde::Deserializer<'de>>(value: D) -> Result<String, D::Error> {
    <String as serde::Deserialize>::deserialize(value).map(|text| escape(&text))
}

#[cfg(test)]
mod tests {
    #[test]
    fn layout_controls_are_visible_and_ordinary_text_is_preserved() {
        assert_eq!(
            super::escape(
                "safe\u{202e}delete\u{200b}\u{0085}\u{2028}\u{2066}\u{feff}\u{e0001}\n\t🙂"
            ),
            "safe\\u202edelete\\u200b\\u0085\\u2028\\u2066\\ufeff\\udb40\\udc01\n\t🙂"
        );
    }

    #[test]
    fn a_line_ending_keeps_no_carriage_return_and_a_lone_one_stays_visible() {
        assert_eq!(super::escape("one\r\ntwo\r\n"), "one\ntwo\n");
        assert_eq!(super::escape("12%\r34%"), "12%\\u000d34%");
    }
}
