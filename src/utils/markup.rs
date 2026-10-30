use std::sync::OnceLock;

use regex::Regex;

use crate::i18n::gettext_f;

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

/// A block of message text: plain paragraphs interleaved with code blocks, quotes,
/// photos, and separators, in text order.
#[derive(Debug, Clone)]
pub(crate) enum RichBlock {
    Paragraph(RichInline),
    Code {
        code: String,
        language: String,
    },
    Quote {
        inline: RichInline,
        expandable: bool,
    },
    /// A photo embedded in a rich message (`pageBlockPhoto`).
    Photo {
        photo: Box<tdlib::types::Photo>,
        caption: RichInline,
    },
    /// A visual separator between rich-message sections.
    Divider,
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

    // Clamp in i64, like `active_entities`, so a malicious offset + length can't overflow
    // i32. The results lie within [u16_start, u16_end], so the casts back to i32 are safe.
    let (u16_start, u16_end) = (u16_start as i64, u16_end as i64);
    let entities = formatted
        .entities
        .iter()
        .filter(|entity| !is_block_entity(&entity.r#type))
        .filter_map(|entity| {
            let entity_start = entity.offset.max(0) as i64;
            let start = entity_start.max(u16_start);
            let end = (entity_start + entity.length.max(0) as i64).min(u16_end);
            (start < end).then(|| tdlib::types::TextEntity {
                offset: (start - u16_start) as i32,
                length: (end - start) as i32,
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
            CustomEmoji(data) => inline
                .custom_emoji
                .push((range.0, range.1, data.custom_emoji_id)),
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

    // The active entity set only changes at an entity's start or end offset, so recompute
    // it only when the cursor crosses one of those boundaries instead of on every char
    // (which reallocated and re-sorted per character).
    let mut boundaries: Vec<i64> = entities
        .iter()
        .flat_map(|entity| {
            let start = entity.offset.max(0) as i64;
            [start, start + entity.length.max(0) as i64]
        })
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();
    let mut next_boundary = 0;
    let mut recompute = true;

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
        while next_boundary < boundaries.len()
            && code_units_offset as i64 >= boundaries[next_boundary]
        {
            recompute = true;
            next_boundary += 1;
        }
        if recompute {
            let active = active_entities(entities, code_units_offset);
            if active != current {
                flush(&mut output, &mut buffer, &current);
                current = active;
            }
            recompute = false;
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
            gettext_f(
                "{hours} h {minutes} min {seconds} s",
                &[
                    ("hours", &hours.to_string()),
                    ("minutes", &minutes.to_string()),
                    ("seconds", &seconds.to_string()),
                ],
            )
        } else {
            gettext_f(
                "{hours} h {seconds} s",
                &[
                    ("hours", &hours.to_string()),
                    ("seconds", &seconds.to_string()),
                ],
            )
        }
    } else {
        let minutes = seconds / 60;
        if minutes > 0 {
            seconds %= 60;
            if seconds > 0 {
                gettext_f(
                    "{minutes} min {seconds} s",
                    &[
                        ("minutes", &minutes.to_string()),
                        ("seconds", &seconds.to_string()),
                    ],
                )
            } else {
                gettext_f("{minutes} min", &[("minutes", &minutes.to_string())])
            }
        } else {
            gettext_f("{seconds} s", &[("seconds", &seconds.to_string())])
        }
    }
}

/// Flattens a `RichText` tree (the format of Telegram's rich messages) into the
/// `FormattedText` the entity pipeline already renders. Styles without a matching
/// entity (subscript, marked, fixed) keep their text; icons and anchors contribute
/// nothing.
pub(crate) fn rich_text_to_formatted(rich: &tdlib::enums::RichText) -> tdlib::types::FormattedText {
    let mut text = String::new();
    let mut entities = Vec::new();
    rich_text_into(rich, &mut text, &mut entities);
    tdlib::types::FormattedText { text, entities }
}

fn rich_text_into(
    rich: &tdlib::enums::RichText,
    text: &mut String,
    entities: &mut Vec<tdlib::types::TextEntity>,
) {
    use tdlib::enums::RichText as Rt;
    use tdlib::enums::TextEntityType as Et;

    // Append `inner` wrapped in one entity spanning exactly what it produced.
    macro_rules! styled {
        ($inner:expr, $type:expr) => {{
            let start = utf16_len(text);
            rich_text_into($inner, text, entities);
            let length = utf16_len(text) - start;
            if length > 0 {
                entities.push(tdlib::types::TextEntity {
                    offset: start,
                    length,
                    r#type: $type,
                });
            }
        }};
    }

    match rich {
        Rt::Plain(data) => text.push_str(&data.text),
        Rt::Bold(data) => styled!(&data.text, Et::Bold),
        Rt::Italic(data) => styled!(&data.text, Et::Italic),
        Rt::Underline(data) => styled!(&data.text, Et::Underline),
        Rt::Strikethrough(data) => styled!(&data.text, Et::Strikethrough),
        Rt::Spoiler(data) => styled!(&data.text, Et::Spoiler),
        Rt::Subscript(data) => rich_text_into(&data.text, text, entities),
        Rt::Superscript(data) => rich_text_into(&data.text, text, entities),
        Rt::Marked(data) => rich_text_into(&data.text, text, entities),
        Rt::Fixed(data) => styled!(&data.text, Et::Code),
        Rt::DateTime(data) => styled!(
            &data.text,
            Et::DateTime(tdlib::types::TextEntityTypeDateTime {
                unix_time: data.unix_time,
                formatting_type: data.formatting_type.clone(),
            })
        ),
        Rt::Mention(data) => styled!(&data.text, Et::Mention),
        Rt::Hashtag(data) => styled!(&data.text, Et::Hashtag),
        Rt::Cashtag(data) => styled!(&data.text, Et::Cashtag),
        Rt::BankCardNumber(data) => styled!(&data.text, Et::BankCardNumber),
        Rt::BotCommand(data) => styled!(&data.text, Et::BotCommand),
        Rt::MentionName(data) => styled!(
            &data.text,
            Et::MentionName(tdlib::types::TextEntityTypeMentionName {
                user_id: data.user_id,
            })
        ),
        Rt::Url(data) => styled!(
            &data.text,
            Et::TextUrl(tdlib::types::TextEntityTypeTextUrl {
                url: data.url.clone(),
            })
        ),
        Rt::EmailAddress(data) => styled!(&data.text, Et::EmailAddress),
        Rt::PhoneNumber(data) => styled!(&data.text, Et::PhoneNumber),
        Rt::CustomEmoji(data) => {
            let start = utf16_len(text);
            text.push_str(&data.alternative_text);
            entities.push(tdlib::types::TextEntity {
                offset: start,
                length: utf16_len(text) - start,
                r#type: Et::CustomEmoji(tdlib::types::TextEntityTypeCustomEmoji {
                    custom_emoji_id: data.custom_emoji_id,
                }),
            });
        }
        Rt::Icon(_) => {}
        Rt::MathematicalExpression(data) => text.push_str(&data.expression),
        Rt::Diff(data) => rich_text_into(&data.text, text, entities),
        Rt::Reference(data) => rich_text_into(&data.text, text, entities),
        Rt::ReferenceLink(data) => styled!(
            &data.text,
            Et::TextUrl(tdlib::types::TextEntityTypeTextUrl {
                url: data.url.clone(),
            })
        ),
        Rt::Anchor(_) => {}
        Rt::AnchorLink(data) => rich_text_into(&data.text, text, entities),
        // Concatenation container: bots wrap whole paragraphs in it, so its
        // children must be walked — dropping it silently blanks the message.
        Rt::RichTexts(data) => {
            for inner in &data.texts {
                rich_text_into(inner, text, entities);
            }
        }
    }
}

/// The plain text of a `RichText` tree (code blocks, table cells).
fn rich_text_plain(rich: &tdlib::enums::RichText) -> String {
    rich_text_to_formatted(rich).text
}

/// Converts a rich message (`MessageRichMessage`, the format Telegram uses for
/// messages with tables, headings, embedded photos, and other block-level
/// formatting) into the block list the bubble already renders.
pub(crate) fn rich_message_to_blocks(rich: &tdlib::types::RichMessage) -> Vec<RichBlock> {
    let mut blocks = Vec::new();
    rich_blocks_into(&rich.blocks, &mut blocks);
    blocks
}

/// The full text of a rich message (copy, export): block texts joined by newlines.
pub(crate) fn rich_message_plain(rich: &tdlib::types::RichMessage) -> String {
    rich_message_to_blocks(rich)
        .into_iter()
        .filter_map(|block| match block {
            RichBlock::Paragraph(inline) => Some(inline.plain),
            RichBlock::Code { code, .. } => Some(code),
            RichBlock::Quote { inline, .. } => Some(inline.plain),
            RichBlock::Photo { caption, .. } => Some(caption.plain),
            RichBlock::Divider => None,
        })
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The first text of a rich message — enough for a chat-list excerpt.
pub(crate) fn rich_message_summary(rich: &tdlib::types::RichMessage) -> String {
    rich_message_to_blocks(rich)
        .into_iter()
        .find_map(|block| match block {
            RichBlock::Paragraph(inline) if !inline.plain.trim().is_empty() => Some(inline.plain),
            RichBlock::Code { code, .. } if !code.trim().is_empty() => Some(code),
            RichBlock::Quote { inline, .. } if !inline.plain.trim().is_empty() => {
                Some(inline.plain)
            }
            _ => None,
        })
        .unwrap_or_default()
}

fn rich_blocks_into(blocks: &[tdlib::enums::PageBlock], out: &mut Vec<RichBlock>) {
    use tdlib::enums::PageBlock as Pb;

    for block in blocks {
        match block {
            Pb::Paragraph(data) => push_paragraph(&data.text, false, out),
            // Headings read as bold paragraphs; tdesktop uses larger fonts, the
            // emphasis is what carries the structure.
            Pb::Title(data) => push_paragraph(&data.title, true, out),
            Pb::Header(data) => push_paragraph(&data.header, true, out),
            Pb::Subheader(data) => push_paragraph(&data.subheader, true, out),
            Pb::SectionHeading(data) => push_paragraph(&data.text, true, out),
            Pb::Kicker(data) => push_paragraph(&data.kicker, true, out),
            Pb::Subtitle(data) => push_paragraph(&data.subtitle, false, out),
            Pb::AuthorDate(data) => push_paragraph(&data.author, false, out),
            Pb::Footer(data) => push_paragraph(&data.footer, false, out),
            Pb::Preformatted(data) => out.push(RichBlock::Code {
                code: rich_text_plain(&data.text),
                language: data.language.clone(),
            }),
            Pb::MathematicalExpression(data) => out.push(RichBlock::Code {
                code: data.expression.clone(),
                language: String::new(),
            }),
            Pb::BlockQuote(data) => {
                let inline = quote_inline(&data.blocks, data.credit.as_ref());
                out.push(RichBlock::Quote {
                    inline,
                    expandable: false,
                });
            }
            Pb::PullQuote(data) => {
                let mut formatted = rich_text_to_formatted(&data.text);
                if let Some(credit) = &data.credit {
                    append_formatted(&mut formatted, "\n— ", credit);
                }
                out.push(RichBlock::Quote {
                    inline: parse_inline(&formatted),
                    expandable: false,
                });
            }
            Pb::List(data) => {
                for item in &data.items {
                    let mut item_blocks = Vec::new();
                    rich_blocks_into(&item.blocks, &mut item_blocks);
                    // The label ("1.", "•") prefixes the item's first paragraph;
                    // anything else keeps its own block.
                    let mut prefixed = false;
                    for block in item_blocks {
                        match block {
                            RichBlock::Paragraph(mut inline) if !prefixed => {
                                inline.plain = format!("{} {}", item.label, inline.plain);
                                inline.markup =
                                    format!("{} {}", escape(&item.label), inline.markup);
                                prefixed = true;
                                out.push(RichBlock::Paragraph(inline));
                            }
                            other => out.push(other),
                        }
                    }
                }
            }
            Pb::Divider => out.push(RichBlock::Divider),
            Pb::Details(data) => rich_blocks_into(&data.blocks, out),
            // Tables render row by row with cell separators: the content stays
            // readable even though the grid itself is flattened.
            Pb::Table(data) => {
                for row in &data.cells {
                    let mut formatted = tdlib::types::FormattedText {
                        text: String::new(),
                        entities: Vec::new(),
                    };
                    for (index, cell) in row.iter().enumerate() {
                        if index > 0 {
                            formatted.text.push_str("  |  ");
                        }
                        if let Some(text) = &cell.text {
                            append_formatted(&mut formatted, "", text);
                        }
                    }
                    if !formatted.text.trim().is_empty() {
                        out.push(RichBlock::Paragraph(parse_inline(&formatted)));
                    }
                }
            }
            Pb::Photo(data) => {
                if let Some(photo) = &data.photo {
                    let caption = data
                        .caption
                        .as_ref()
                        .map(|caption| parse_inline(&rich_text_to_formatted(&caption.text)))
                        .unwrap_or_default();
                    out.push(RichBlock::Photo {
                        photo: Box::new(photo.clone()),
                        caption,
                    });
                }
            }
            // Media blocks other than photos keep their caption text; the media
            // itself needs players this view does not embed inline.
            Pb::Video(data) => {
                if let Some(caption) = &data.caption {
                    push_paragraph(&caption.text, false, out);
                }
            }
            Pb::Animation(data) => {
                if let Some(caption) = &data.caption {
                    push_paragraph(&caption.text, false, out);
                }
            }
            Pb::Audio(data) => {
                if let Some(caption) = &data.caption {
                    push_paragraph(&caption.text, false, out);
                }
            }
            Pb::VoiceNote(data) => {
                if let Some(caption) = &data.caption {
                    push_paragraph(&caption.text, false, out);
                }
            }
            _ => {}
        }
    }
}

fn push_paragraph(rich: &tdlib::enums::RichText, bold: bool, out: &mut Vec<RichBlock>) {
    let mut formatted = rich_text_to_formatted(rich);
    if bold {
        let length = utf16_len(&formatted.text);
        if length > 0 {
            formatted.entities.push(tdlib::types::TextEntity {
                offset: 0,
                length,
                r#type: tdlib::enums::TextEntityType::Bold,
            });
        }
    }
    if !formatted.text.trim().is_empty() {
        out.push(RichBlock::Paragraph(parse_inline(&formatted)));
    }
}

/// The quote body: its inner blocks joined into one inline run, plus the credit
/// line prefixed with an em dash.
fn quote_inline(
    blocks: &[tdlib::enums::PageBlock],
    credit: Option<&tdlib::enums::RichText>,
) -> RichInline {
    let mut body = Vec::new();
    rich_blocks_into(blocks, &mut body);
    let mut formatted = tdlib::types::FormattedText {
        text: String::new(),
        entities: Vec::new(),
    };
    for block in body {
        match block {
            RichBlock::Paragraph(inline) => {
                if !formatted.text.is_empty() {
                    formatted.text.push('\n');
                }
                formatted.text.push_str(&inline.plain);
            }
            RichBlock::Code { code, .. } => {
                if !formatted.text.is_empty() {
                    formatted.text.push('\n');
                }
                formatted.text.push_str(&code);
            }
            RichBlock::Quote { inline, .. } => {
                if !formatted.text.is_empty() {
                    formatted.text.push('\n');
                }
                formatted.text.push_str(&inline.plain);
            }
            _ => {}
        }
    }
    if let Some(credit) = credit {
        append_formatted(&mut formatted, "\n— ", credit);
    }
    parse_inline(&formatted)
}

/// Appends `prefix` + the plain text of `rich` to `target`, re-basing `rich`'s
/// entities onto the joined string.
fn append_formatted(
    target: &mut tdlib::types::FormattedText,
    prefix: &str,
    rich: &tdlib::enums::RichText,
) {
    let added = rich_text_to_formatted(rich);
    let offset = utf16_len(&target.text) + utf16_len(prefix);
    target.text.push_str(prefix);
    target.text.push_str(&added.text);
    for mut entity in added.entities {
        entity.offset += offset;
        target.entities.push(entity);
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
                    CustomEmoji(tdlib::types::TextEntityTypeCustomEmoji { custom_emoji_id: 7 }),
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

    #[test]
    fn rich_texts_container_flattens_into_the_paragraph() {
        use tdlib::enums::PageBlock;
        use tdlib::enums::RichText;
        use tdlib::types::PageBlockParagraph;
        use tdlib::types::RichMessage;
        use tdlib::types::RichTextBold;
        use tdlib::types::RichTextPlain;
        use tdlib::types::RichTextUrl;
        use tdlib::types::RichTexts;

        let plain = |text: &str| RichText::Plain(RichTextPlain { text: text.into() });
        // The shape ChatLogixBot sends: one paragraph whose text is a
        // `richTexts` container of styled runs.
        let blocks = rich_message_to_blocks(&RichMessage {
            blocks: vec![PageBlock::Paragraph(PageBlockParagraph {
                text: RichText::RichTexts(RichTexts {
                    texts: vec![
                        RichText::Bold(Box::new(RichTextBold {
                            text: plain("📆 Что обсуждалось "),
                        })),
                        plain("\nВсего было написано "),
                        RichText::Url(Box::new(RichTextUrl {
                            text: plain("6 сообщений"),
                            url: "https://t.me/c/1639516024/67602/98282".into(),
                            is_cached: false,
                        })),
                    ],
                }),
            })],
            is_rtl: false,
            is_full: true,
        });
        let [RichBlock::Paragraph(inline)] = blocks.as_slice() else {
            panic!("expected a single paragraph, got {blocks:?}");
        };
        assert_eq!(
            inline.plain,
            "📆 Что обсуждалось \nВсего было написано 6 сообщений"
        );
        assert!(inline.markup.contains("<b>📆 Что обсуждалось </b>"));
        assert!(
            inline
                .markup
                .contains("<a href='https://t.me/c/1639516024/67602/98282'>6 сообщений</a>")
        );
    }
}
