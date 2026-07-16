use std::sync::OnceLock;

use regex::Regex;

fn protocol_re() -> &'static Regex {
    static PROTOCOL_RE: OnceLock<Regex> = OnceLock::new();
    PROTOCOL_RE.get_or_init(|| Regex::new(r"^\w+://").unwrap())
}

pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
        .replace('"', "&quot;")
}

/// Replace variables in the given string with the given dictionary.
///
/// The expected format to replace is `{name}`, where `name` is the first string
/// in the dictionary entry tuple.
// Function taken from Fractal: https://gitlab.gnome.org/GNOME/fractal/-/blob/main/src/utils.rs
pub(crate) fn freplace(s: String, args: &[(&str, &str)]) -> String {
    let mut s = s;
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

/// Replace successive `{}` placeholders in `msg` with `args`, left to right.
///
/// Unlike a naive `replacen` loop, substituted text is never rescanned, so an
/// argument value that itself contains `{}` cannot capture a later argument.
/// Surplus placeholders are left as literal `{}`; surplus arguments are ignored.
pub(crate) fn positional_replace(msg: String, args: &[String]) -> String {
    let mut result = String::with_capacity(msg.len());
    let mut rest = msg.as_str();
    let mut args = args.iter();

    while let Some(pos) = rest.find("{}") {
        result.push_str(&rest[..pos]);
        match args.next() {
            Some(arg) => result.push_str(arg),
            None => {
                // No argument left for this placeholder; keep the rest verbatim.
                result.push_str(rest);
                return result;
            }
        }
        rest = &rest[pos + 2..];
    }

    result.push_str(rest);
    result
}

pub(crate) fn linkify(text: &str) -> String {
    if !protocol_re().is_match(text) {
        format!("http://{text}")
    } else {
        text.to_string()
    }
}

pub(crate) fn convert_to_markup(text: String, entity: &tdlib::enums::TextEntityType) -> String {
    use tdlib::enums::TextEntityType::*;

    match entity {
        Url => format!("<a href='{}'>{}</a>", linkify(&text), text),
        EmailAddress => format!("<a href='mailto:{text}'>{text}</a>"),
        PhoneNumber => format!("<a href='tel:{text}'>{text}</a>"),
        Bold => format!("<b>{text}</b>"),
        Italic => format!("<i>{text}</i>"),
        Underline => format!("<u>{text}</u>"),
        Strikethrough => format!("<s>{text}</s>"),
        Code | Pre | PreCode(_) => format!("<tt>{text}</tt>"),
        TextUrl(data) => format!("<a href='{}'>{}</a>", escape(&data.url), text),
        _ => text,
    }
}

pub(crate) fn parse_formatted_text(formatted_text: tdlib::types::FormattedText) -> String {
    let entities = &formatted_text.entities;

    let mut output = String::new();
    let mut buffer = String::new();
    // The entities active over the characters currently in `buffer`, outermost first.
    let mut current: Vec<usize> = Vec::new();

    // Entity offsets/lengths are in utf16 code units, not code points, so we track the
    // offset in utf16 units to line up with them.
    let mut code_units_offset: usize = 0;

    let flush = |output: &mut String, buffer: &mut String, active: &[usize]| {
        if buffer.is_empty() {
            return;
        }
        // Wrap innermost-last so the first (outermost) entity ends up outermost.
        let mut text = escape(buffer);
        for &i in active.iter().rev() {
            text = convert_to_markup(text, &entities[i].r#type);
        }
        output.push_str(&text);
        buffer.clear();
    };

    for c in formatted_text.text.chars() {
        let active = active_entities(entities, code_units_offset);
        if active != current {
            flush(&mut output, &mut buffer, &current);
            current = active;
        }

        buffer.push(c);
        code_units_offset += c.len_utf16();
    }

    flush(&mut output, &mut buffer, &current);

    output
}

/// Indices of the entities covering `code_units_offset`, ordered outermost first
/// (earliest start, then longest). Correctly handles nested and partially
/// overlapping entities, and computes ranges in `i64` to avoid `i32` overflow on
/// malicious `offset`/`length` values.
fn active_entities(entities: &[tdlib::types::TextEntity], code_units_offset: usize) -> Vec<usize> {
    let pos = code_units_offset as i64;

    let mut active: Vec<usize> = entities
        .iter()
        .enumerate()
        .filter_map(|(i, entity)| {
            let start = entity.offset.max(0) as i64;
            let end = start + entity.length.max(0) as i64;
            (pos >= start && pos < end).then_some(i)
        })
        .collect();

    active.sort_by(|&a, &b| {
        let (a, b) = (&entities[a], &entities[b]);
        a.offset
            .cmp(&b.offset)
            .then_with(|| b.length.cmp(&a.length))
    });

    active
}

pub(crate) fn human_friendly_duration(mut seconds: i32) -> String {
    let hours = seconds / (60 * 60);
    if hours > 0 {
        seconds %= 60 * 60;
        let minutes = seconds / 60;
        if minutes > 0 {
            seconds %= 60;
            gettext!("{} h {} min {} s", hours, minutes, seconds)
        } else {
            gettext!("{} h {} s", hours, seconds)
        }
    } else {
        let minutes = seconds / 60;
        if minutes > 0 {
            seconds %= 60;
            if seconds > 0 {
                gettext!("{} min {} s", minutes, seconds)
            } else {
                gettext!("{} min", minutes)
            }
        } else {
            gettext!("{} s", seconds)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(
        offset: i32,
        length: i32,
        r#type: tdlib::enums::TextEntityType,
    ) -> tdlib::types::TextEntity {
        tdlib::types::TextEntity {
            offset,
            length,
            r#type,
        }
    }

    #[test]
    fn formatted_text_uses_utf16_entity_offsets() {
        use tdlib::enums::TextEntityType::Bold;
        use tdlib::enums::TextEntityType::Italic;

        let cases = [
            ("a😀b", vec![entity(1, 2, Bold)], "a<b>😀</b>b"),
            (
                "😀bolditalic",
                vec![entity(2, 4, Bold), entity(6, 6, Italic)],
                "😀<b>bold</b><i>italic</i>",
            ),
            (
                "😀 plain 𝄞bold<&",
                vec![entity(11, 6, Bold)],
                "😀 plain 𝄞<b>bold&lt;&amp;</b>",
            ),
        ];

        for (text, entities, expected) in cases {
            let formatted_text = tdlib::types::FormattedText {
                text: text.to_owned(),
                entities,
            };

            assert_eq!(parse_formatted_text(formatted_text), expected, "{text:?}");
        }
    }

    #[test]
    fn nested_and_overlapping_entities() {
        use tdlib::enums::TextEntityType::Bold;
        use tdlib::enums::TextEntityType::Italic;

        let cases = [
            // Italic word nested inside a bold phrase (differing offsets). The outer
            // bold is closed and reopened at the inner boundary; verbose but valid and
            // rendered identically.
            (
                "bold italic",
                vec![entity(0, 11, Bold), entity(5, 6, Italic)],
                "<b>bold </b><b><i>italic</i></b>",
            ),
            // Same-offset nesting.
            (
                "x",
                vec![entity(0, 1, Bold), entity(0, 1, Italic)],
                "<b><i>x</i></b>",
            ),
            // Partial overlap: bold [0,5), italic [3,8).
            (
                "abcdefgh",
                vec![entity(0, 5, Bold), entity(3, 5, Italic)],
                "<b>abc</b><b><i>de</i></b><i>fgh</i>",
            ),
        ];

        for (text, entities, expected) in cases {
            let formatted_text = tdlib::types::FormattedText {
                text: text.to_owned(),
                entities,
            };

            assert_eq!(parse_formatted_text(formatted_text), expected, "{text:?}");
        }
    }

    #[test]
    fn duration_formatting_boundary_cases() {
        let cases = [
            (-1, "-1 s"),
            (0, "0 s"),
            (59, "59 s"),
            (60, "1 min"),
            (61, "1 min 1 s"),
            (3_599, "59 min 59 s"),
            (3_600, "1 h 0 s"),
            (3_601, "1 h 1 s"),
            (3_660, "1 h 1 min 0 s"),
            (3_661, "1 h 1 min 1 s"),
        ];

        for (seconds, expected) in cases {
            assert_eq!(human_friendly_duration(seconds), expected, "{seconds}");
        }
    }
}
