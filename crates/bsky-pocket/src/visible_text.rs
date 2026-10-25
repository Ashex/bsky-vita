use serde::{Deserialize, Serialize};

pub const VISIBLE_SCHEMA: u8 = 1;
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_ROWS: usize = 3;
pub const MAX_ROW_BYTES: usize = 1024;
pub const MAX_TOTAL_BYTES: usize = 3072;
pub const MAX_MEASURE_CALLS_ROW: usize = 64;
pub const MAX_MEASURE_CALLS_TOTAL: usize = 192;
pub const MAX_MEASURE_BYTES_ROW: usize = 1024;
pub const MAX_MEASURE_BYTES_TOTAL: usize = 3072;
pub const MAX_LINE_BYTES: usize = 128;
pub const MAX_INPUT_BYTES_ROW: usize = 256;
pub const MAX_MESSAGE_BYTES: usize = 4096;
pub const MIN_WIDTH: u16 = 32;
pub const MAX_WIDTH: u16 = 440;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stamp {
    pub auth_generation: u64,
    pub mount_generation: u64,
    pub history_revision: u64,
    pub layout_id: u64,
}
impl Stamp {
    pub fn valid(&self) -> bool {
        self.auth_generation > 0
            && self.mount_generation > 0
            && self.history_revision > 0
            && self.layout_id > 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasureRow {
    pub uri: String,
    pub width: u16,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasureRequest {
    pub schema: u8,
    pub stamp: Stamp,
    pub rows: Vec<MeasureRow>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasureStatus {
    Ready,
    Placeholder,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasureResult {
    pub uri: String,
    pub status: MeasureStatus,
    pub width: u16,
    pub line_count: u8,
    pub line_height: u16,
    pub baseline_offset: u16,
    pub body_height: u16,
    pub continuation: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasureReply {
    pub schema: u8,
    pub stamp: Stamp,
    pub rows: Vec<MeasureResult>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub uri: String,
    pub x: u16,
    pub y: u16,
    pub body_height: u16,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementMessage {
    pub schema: u8,
    pub stamp: Stamp,
    pub rows: Vec<Placement>,
}

fn valid_uri(uri: &str) -> bool {
    !uri.is_empty() && uri.len() <= MAX_ROW_BYTES && !uri.chars().any(char::is_control)
}
impl MeasureRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema != VISIBLE_SCHEMA || !self.stamp.valid() {
            return Err("invalid schema or stamp");
        }
        if self.rows.is_empty() || self.rows.len() > MAX_ROWS {
            return Err("invalid candidate count");
        }
        let mut total = 0usize;
        for row in &self.rows {
            total = total
                .checked_add(row.uri.len())
                .ok_or("candidate size overflow")?;
            if !valid_uri(&row.uri) || !(MIN_WIDTH..=MAX_WIDTH).contains(&row.width) {
                return Err("invalid candidate");
            }
        }
        if total > MAX_TOTAL_BYTES {
            return Err("candidate bytes exceed limit");
        }
        Ok(())
    }
    pub fn parse(raw: &str) -> Result<Self, &'static str> {
        if raw.len() > MAX_MESSAGE_BYTES {
            return Err("message exceeds limit");
        }
        let parsed: Self = serde_json::from_str(raw).map_err(|_| "invalid measure message")?;
        parsed.validate()?;
        Ok(parsed)
    }
}
impl MeasureReply {
    pub fn validate_for(&self, request: &MeasureRequest) -> Result<(), &'static str> {
        if self.schema != VISIBLE_SCHEMA
            || self.stamp != request.stamp
            || self.rows.len() != request.rows.len()
        {
            return Err("stale or mismatched measurement");
        }
        for (got, expected) in self.rows.iter().zip(&request.rows) {
            if got.uri != expected.uri
                || got.width != expected.width
                || got.line_count > 2
                || got.line_height > 256
                || got.baseline_offset > got.line_height
                || got.body_height > 512
            {
                return Err("invalid measurement metrics");
            }
        }
        Ok(())
    }
    pub fn to_bounded_json(&self) -> Result<String, &'static str> {
        let raw = serde_json::to_string(self).map_err(|_| "measurement serialization failed")?;
        if raw.len() > MAX_MESSAGE_BYTES {
            return Err("message exceeds limit");
        }
        Ok(raw)
    }
}
impl PlacementMessage {
    pub fn validate_for(&self, request: &MeasureRequest) -> Result<(), &'static str> {
        if self.schema != VISIBLE_SCHEMA
            || self.stamp != request.stamp
            || self.rows.len() != request.rows.len()
        {
            return Err("stale or mismatched placement");
        }
        for (got, expected) in self.rows.iter().zip(&request.rows) {
            if got.uri != expected.uri || got.body_height > 512 {
                return Err("placement row mismatch or invalid height");
            }
        }
        Ok(())
    }
}

fn is_grapheme_extension(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036f | 0x1ab0..=0x1aff | 0x1dc0..=0x1dff | 0x20d0..=0x20ff | 0xfe00..=0xfe0f | 0xfe20..=0xfe2f | 0x1f3fb..=0x1f3ff | 0xe0100..=0xe01ef)
}

pub fn sanitized_display(source: &str) -> String {
    source
        .chars()
        .map(|c| {
            if c == '\0' || (c.is_control() && c != '\n' && c != '\t') {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect()
}

/// Split a line on UTF-8 boundaries, preferring whitespace. Measurement is supplied
/// by the native font adapter and never sees more than the explicit call/byte budgets.
pub fn wrap_bounded<F>(source: &str, width: u16, mut measure: F) -> (Vec<String>, bool)
where
    F: FnMut(&str) -> u16,
{
    let sanitized = sanitized_display(source);
    let mut input_end = sanitized.len().min(MAX_INPUT_BYTES_ROW);
    while !sanitized.is_char_boundary(input_end) {
        input_end -= 1;
    }
    let clean = &sanitized[..input_end];
    let mut calls = 0usize;
    let mut bytes = 0usize;
    let mut total_calls = 0usize;
    let mut total_bytes = 0usize;
    let mut lines = Vec::new();
    let mut continuation = sanitized.len() > input_end;
    'paragraphs: for paragraph in clean.split('\n') {
        let mut rest = paragraph;
        if rest.is_empty() {
            if lines.len() == 2 {
                continuation = true;
                break;
            }
            lines.push(String::new());
            continue;
        }
        while !rest.is_empty() {
            let mut end = rest.len().min(MAX_LINE_BYTES);
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            if end == 0 {
                continuation = true;
                break 'paragraphs;
            }
            // Test prefixes that could actually be emitted, not suffixes that
            // merely happen to fit. Character boundaries preserve UTF-8.
            let mut best = 0;
            for (i, ch) in rest.char_indices().take_while(|(i, _)| *i < end) {
                let candidate_end = i + ch.len_utf8();
                if calls >= MAX_MEASURE_CALLS_ROW
                    || total_calls >= MAX_MEASURE_CALLS_TOTAL
                    || bytes.saturating_add(candidate_end) > MAX_MEASURE_BYTES_ROW
                    || total_bytes.saturating_add(candidate_end) > MAX_MEASURE_BYTES_TOTAL
                {
                    continuation = true;
                    if best > 0 {
                        break;
                    }
                    break 'paragraphs;
                }
                calls += 1;
                bytes += candidate_end;
                total_bytes += candidate_end;
                total_calls += 1;
                if measure(&rest[..candidate_end]) <= width {
                    best = candidate_end;
                }
            }
            if best == 0 {
                if calls == 0 && lines.is_empty() {
                    best = rest.chars().next().map_or(0, char::len_utf8);
                }
                if best == 0 {
                    continuation = true;
                    break 'paragraphs;
                }
            }
            while best > 0 && !rest.is_char_boundary(best) {
                best -= 1;
            }
            let mut cut = best;
            if cut < rest.len() {
                if let Some(space) = rest[..cut].rfind(char::is_whitespace) {
                    if space > 0 {
                        cut = space;
                    }
                }
            }
            if cut == 0 {
                cut = best;
            }
            // Preserve grapheme clusters (combining marks, variation selectors,
            // emoji modifiers and ZWJ sequences) when cutting visible prefixes.
            while cut < rest.len() && cut > 0 {
                let next = rest[cut..].chars().next().unwrap_or('\0');
                let prev = rest[..cut].chars().next_back().unwrap_or('\0');
                if is_grapheme_extension(next) || prev == '\u{200d}' {
                    cut += next.len_utf8();
                } else {
                    break;
                }
            }
            if cut > MAX_LINE_BYTES {
                cut = best;
                while cut > 0 && !rest.is_char_boundary(cut) {
                    cut -= 1;
                }
            }
            let line = rest[..cut].trim_end().to_owned();
            if lines.len() == 2 {
                continuation = true;
                break 'paragraphs;
            }
            lines.push(line);
            rest = rest[cut..].trim_start();
        }
        if lines.len() == 2 {
            continuation = paragraph != clean.split('\n').last().unwrap_or("");
            break;
        }
    }
    if !continuation && (lines.len() == 2 && clean.split('\n').count() > 2) {
        continuation = true;
    }
    (lines, continuation)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> MeasureRequest {
        MeasureRequest {
            schema: 1,
            stamp: Stamp {
                auth_generation: 1,
                mount_generation: 2,
                history_revision: 3,
                layout_id: 4,
            },
            rows: vec![MeasureRow {
                uri: "at://one".into(),
                width: 128,
            }],
        }
    }
    #[test]
    fn protocol_rejects_unknown_oversized_stale_and_misaligned_messages() {
        let req = request();
        let mut reply = MeasureReply {
            schema: 1,
            stamp: req.stamp.clone(),
            rows: vec![MeasureResult {
                uri: "at://one".into(),
                status: MeasureStatus::Ready,
                width: 128,
                line_count: 1,
                line_height: 12,
                baseline_offset: 9,
                body_height: 12,
                continuation: false,
            }],
        };
        reply.validate_for(&req).unwrap();
        reply.stamp.layout_id += 1;
        assert!(reply.validate_for(&req).is_err());
        assert!(MeasureRequest::parse(&format!("{} ", " ".repeat(MAX_MESSAGE_BYTES))).is_err());
        assert!(MeasureRequest::parse(r#"{"schema":1,"stamp":{},"rows":[],"extra":0}"#).is_err());
    }
    #[test]
    fn wrapping_measures_emitted_prefixes_and_caps_multibyte_continuation() {
        let source = format!("{} {}", "a".repeat(8), "x".repeat(100));
        let mut measured = Vec::new();
        let (lines, continuation) = wrap_bounded(&source, 100, |candidate| {
            assert!(source.contains(candidate));
            measured.push(candidate.to_owned());
            (candidate.chars().count() * 5) as u16
        });
        assert!(lines.len() <= 2);
        assert!(!lines.is_empty());
        assert!(measured.iter().all(|candidate| !candidate.is_empty()));
        assert!(lines.iter().all(|line| {
            !line.is_empty() && line.len() <= MAX_LINE_BYTES && line.chars().count() * 5 <= 100
        }));
        assert!(continuation);
        assert!(measured.len() <= MAX_MEASURE_CALLS_ROW);
        assert!(measured.iter().map(String::len).sum::<usize>() <= MAX_MEASURE_BYTES_ROW);
    }

    #[test]
    fn wrapping_sanitizes_nul_keeps_utf8_and_bounds_to_two_lines() {
        let source = format!(
            "{}\n{}",
            "🛰e\u{301}\0".repeat(150),
            "https://example.test/".repeat(100)
        );
        let (lines, more) = wrap_bounded(&source, 32, |_| 33);
        assert!(lines.len() <= 2);
        assert!(lines
            .iter()
            .all(|line| line.len() <= MAX_LINE_BYTES && !line.as_bytes().contains(&0)));
        assert!(more);
        assert_eq!(source.as_bytes().contains(&0), true);
    }
}
