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
        // In-app schemes, resolved by MessageLabel's activate-link handler.
        Mention => format!("<a href='mention:{text}'>{text}</a>"),
        MentionName(data) => format!("<a href='mention-id:{}'>{text}</a>", data.user_id),
        Hashtag | Cashtag => format!("<a href='tag:{text}'>{text}</a>"),
        BotCommand => format!("<a href='command:{text}'>{text}</a>"),
        _ => text,
    }
}

/// One inline-formatted run of message text: Pango markup for a label, plus the byte
/// ranges (into `plain`) of the entities that need treatment beyond markup — spoilers
/// (hidden behind an animated overlay), inline code (click to copy), and custom emoji
/// (drawn from their sticker files).
#[derive(Debug, Clone, Default)]
pub(crate) struct RichInline {
    pub(crate) plain: String,
    pub(crate) markup: String,
    pub(crate) spoilers: Vec<(usize, usize)>,
    pub(crate) codes: Vec<(usize, usize)>,
    pub(crate) custom_emoji: Vec<(usize, usize, i64)>,
}

/// A block of message text: plain paragraphs interleaved with code blocks and quotes,
/// in text order.
#[derive(Debug, Clone)]
pub(crate) enum RichBlock {
    Paragraph(RichInline),
    Code { code: String, language: String },
    Quote { inline: RichInline, expandable: bool },
}

/// Converts a TDLib utf16 code-unit range into a byte range of `text`, `None` if the
/// range is out of bounds.
fn utf16_range_to_bytes(text: &str, offset: i32, length: i32) -> Option<(usize, usize)> {
    let start_u16 = offset.max(0) as usize;
    let end_u16 = start_u16 + length.max(0) as usize;

    let mut u16_pos = 0;
    let mut start = None;
    for (byte_idx, c) in text.char_indices() {
        if start.is_none() && u16_pos >= start_u16 {
            start = Some(byte_idx);
        }
        if u16_pos >= end_u16 {
            return start.map(|s| (s, byte_idx));
        }
        u16_pos += c.len_utf16();
    }
    if start.is_none() && u16_pos >= start_u16 {
        start = Some(text.len());
    }
    (u16_pos >= end_u16)
        .then_some(start)
        .flatten()
        .map(|s| (s, text.len()))
}

fn utf16_len(text: &str) -> i32 {
    text.chars().map(|c| c.len_utf16() as i32).sum()
}

/// Whether an entity is block-level (rendered as its own widget, not inline markup).
fn is_block_entity(entity: &tdlib::enums::TextEntityType) -> bool {
    use tdlib::enums::TextEntityType::*;
    matches!(entity, Pre | PreCode(_) | BlockQuote | ExpandableBlockQuote)
}

/// The sub-`FormattedText` covering `byte_start..byte_end`, with the intersecting
/// inline entities clamped and re-based (block-level entities are dropped — the caller
/// renders those as separate widgets).
fn slice_formatted(
    formatted: &tdlib::types::FormattedText,
    byte_start: usize,
    byte_end: usize,
) -> tdlib::types::FormattedText {
    let u16_start = utf16_len(&formatted.text[..byte_start]);
    let u16_end = u16_start + utf16_len(&formatted.text[byte_start..byte_end]);

    let entities = formatted
        .entities
        .iter()
        .filter(|entity| !is_block_entity(&entity.r#type))
        .filter_map(|entity| {
            let start = entity.offset.max(u16_start);
            let end = (entity.offset + entity.length).min(u16_end);
            (start < end).then(|| tdlib::types::TextEntity {
                offset: start - u16_start,
                length: end - start,
                r#type: entity.r#type.clone(),
            })
        })
        .collect();

    tdlib::types::FormattedText {
        text: formatted.text[byte_start..byte_end].to_string(),
        entities,
    }
}

/// Renders a `FormattedText` into a [`RichInline`]: Pango markup plus the byte ranges
/// of spoilers, inline code, and custom emoji.
pub(crate) fn parse_inline(formatted: &tdlib::types::FormattedText) -> RichInline {
    use tdlib::enums::TextEntityType::*;

    let mut inline = RichInline {
        plain: formatted.text.clone(),
        markup: parse_formatted_text(formatted.clone()),
        ..Default::default()
    };

    for entity in &formatted.entities {
        let Some(range) = utf16_range_to_bytes(&formatted.text, entity.offset, entity.length)
        else {
            continue;
        };
        match &entity.r#type {
            Spoiler => inline.spoilers.push(range),
            Code => inline.codes.push(range),
            CustomEmoji(data) => {
                inline
                    .custom_emoji
                    .push((range.0, range.1, data.custom_emoji_id))
            }
            _ => {}
        }
    }

    inline
}

/// Splits a message's `FormattedText` into ordered blocks: paragraphs, code blocks
/// (`Pre`/`PreCode`), and block quotes. Telegram never nests or overlaps block-level
/// entities; overlapping ones (malformed input) are dropped defensively.
pub(crate) fn parse_rich_text(formatted: &tdlib::types::FormattedText) -> Vec<RichBlock> {
    use tdlib::enums::TextEntityType::*;

    let mut block_entities: Vec<(usize, usize, &tdlib::types::TextEntity)> = formatted
        .entities
        .iter()
        .filter(|entity| is_block_entity(&entity.r#type))
        .filter_map(|entity| {
            utf16_range_to_bytes(&formatted.text, entity.offset, entity.length)
                .map(|(start, end)| (start, end, entity))
        })
        .collect();
    block_entities.sort_by_key(|&(start, ..)| start);

    let mut blocks = Vec::new();
    let mut cursor = 0;

    // The plain text between blocks, with the single newline that delimits it from an
    // adjacent block trimmed away (it is layout, not content).
    let push_paragraph = |blocks: &mut Vec<RichBlock>, mut start: usize, mut end: usize| {
        if start > 0 && formatted.text[start..end].starts_with('\n') {
            start += 1;
        }
        if end < formatted.text.len() && formatted.text[start..end].ends_with('\n') {
            end -= 1;
        }
        if start < end {
            blocks.push(RichBlock::Paragraph(parse_inline(&slice_formatted(
                formatted, start, end,
            ))));
        }
    };

    for (start, end, entity) in block_entities {
        if start < cursor {
            continue; // overlapping block entity, malformed
        }
        push_paragraph(&mut blocks, cursor, start);
        let block = match &entity.r#type {
            Pre => RichBlock::Code {
                code: formatted.text[start..end].to_string(),
                language: String::new(),
            },
            PreCode(data) => RichBlock::Code {
                code: formatted.text[start..end].to_string(),
                language: data.language.clone(),
            },
            BlockQuote | ExpandableBlockQuote => RichBlock::Quote {
                inline: parse_inline(&slice_formatted(formatted, start, end)),
                expandable: matches!(&entity.r#type, ExpandableBlockQuote),
            },
            _ => unreachable!(),
        };
        blocks.push(block);
        cursor = end;
    }
    push_paragraph(&mut blocks, cursor, formatted.text.len());

    blocks
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
    fn rich_blocks_split_and_trim_boundary_newlines() {
        use tdlib::enums::TextEntityType::BlockQuote;
        use tdlib::enums::TextEntityType::Pre;

        let formatted = tdlib::types::FormattedText {
            text: "intro\ncode here\noutro".to_owned(),
            entities: vec![entity(6, 9, Pre)],
        };
        match &parse_rich_text(&formatted)[..] {
            [
                RichBlock::Paragraph(a),
                RichBlock::Code { code, language },
                RichBlock::Paragraph(b),
            ] => {
                assert_eq!(a.plain, "intro");
                assert_eq!(code, "code here");
                assert!(language.is_empty());
                assert_eq!(b.plain, "outro");
            }
            other => panic!("unexpected blocks: {other:?}"),
        }

        // A quote at the very start, no surrounding paragraphs.
        let formatted = tdlib::types::FormattedText {
            text: "quoted".to_owned(),
            entities: vec![entity(0, 6, BlockQuote)],
        };
        match &parse_rich_text(&formatted)[..] {
            [RichBlock::Quote { inline, expandable }] => {
                assert_eq!(inline.plain, "quoted");
                assert!(!expandable);
            }
            other => panic!("unexpected blocks: {other:?}"),
        }
    }

    #[test]
    fn rich_inline_byte_ranges_from_utf16_offsets() {
        use tdlib::enums::TextEntityType::Code;
        use tdlib::enums::TextEntityType::CustomEmoji;
        use tdlib::enums::TextEntityType::Spoiler;

        let formatted = tdlib::types::FormattedText {
            // "😀" is 2 utf16 units / 4 bytes.
            text: "😀 secret x".to_owned(),
            entities: vec![
                entity(
                    0,
                    2,
                    CustomEmoji(tdlib::types::TextEntityTypeCustomEmoji {
                        custom_emoji_id: 7,
                    }),
                ),
                entity(3, 6, Spoiler),
                entity(10, 1, Code),
            ],
        };
        let inline = parse_inline(&formatted);
        assert_eq!(inline.custom_emoji, vec![(0, 4, 7)]);
        assert_eq!(inline.spoilers, vec![(5, 11)]);
        assert_eq!(&inline.plain[5..11], "secret");
        assert_eq!(inline.codes, vec![(12, 13)]);
        assert_eq!(&inline.plain[12..13], "x");
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
