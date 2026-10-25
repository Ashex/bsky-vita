use serde::{Deserialize, Serialize};

pub const VISIBLE_SCHEMA: u8 = 1;
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_ROWS: usize = 200;
pub const MAX_MEASURE_BATCH: usize = 3;
pub const VIEWPORT_WIDTH: u32 = 480;
pub const VIEWPORT_HEIGHT: u32 = 272;
pub const VIEWPORT_TOP: i64 = 40;
pub const VIEWPORT_BOTTOM: i64 = 264;
pub const MAX_LAYOUT_ROWS: usize = 200;
pub const MAX_ROW_BYTES: usize = 1024;
pub const MAX_TOTAL_BYTES: usize = 8192;
pub const MAX_MEASURE_CALLS_ROW: usize = 256;
pub const MAX_MEASURE_CALLS_TOTAL: usize = 64;
pub const MAX_MEASURE_BYTES_ROW: usize = 8 * 1024;
pub const MAX_MEASURE_BYTES_TOTAL: usize = 8 * 1024;
pub const MAX_LINE_BYTES: usize = 512;
pub const MAX_INPUT_BYTES_ROW: usize = MAX_TEXT_BYTES;
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
    pub line_count: u32,
    pub line_height: u16,
    pub baseline_offset: u16,
    pub body_height: u32,
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
    pub y: i32,
    pub body_height: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementMessage {
    pub schema: u8,
    pub stamp: Stamp,
    pub rows: Vec<Placement>,
}

pub fn validate_window(history: &[String], proposed: &[String]) -> Option<Vec<String>> {
    if proposed.is_empty() || proposed.len() > MAX_LAYOUT_ROWS {
        return None;
    }
    let start = history.iter().position(|uri| uri == &proposed[0])?;
    let expected = history.get(start..start.checked_add(proposed.len())?)?;
    expected
        .iter()
        .zip(proposed)
        .all(|(a, b)| a == b)
        .then(|| proposed.to_vec())
}
fn valid_uri(uri: &str) -> bool {
    !uri.is_empty() && uri.len() <= MAX_ROW_BYTES && !uri.chars().any(char::is_control)
}
impl MeasureRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema != VISIBLE_SCHEMA || !self.stamp.valid() {
            return Err("invalid schema or stamp");
        }
        if self.rows.len() > MAX_LAYOUT_ROWS {
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
        let v: Self = serde_json::from_str(raw).map_err(|_| "invalid measure message")?;
        v.validate()?;
        Ok(v)
    }
}
impl MeasureReply {
    pub fn validate_for(&self, request: &MeasureRequest) -> Result<(), &'static str> {
        if self.schema != VISIBLE_SCHEMA
            || self.stamp != request.stamp
            || self.rows.is_empty()
            || self.rows.len() > request.rows.len()
        {
            return Err("stale or mismatched measurement");
        }
        for (got, expected) in self.rows.iter().zip(&request.rows) {
            if got.uri != expected.uri
                || got.width != expected.width
                || got.line_count > MAX_TEXT_BYTES as u32
                || got.line_height > 256
                || got.baseline_offset > got.line_height
                || got.body_height > MAX_TEXT_BYTES as u32 * 256
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
    pub fn validate_for(
        &self,
        request: &MeasureRequest,
        known: &[MeasureResult],
    ) -> Result<(), &'static str> {
        if self.schema != VISIBLE_SCHEMA
            || self.stamp != request.stamp
            || self.rows.is_empty()
            || self.rows.len() != request.rows.len()
            || self.rows.len() > MAX_LAYOUT_ROWS
            || request.rows.len() > MAX_LAYOUT_ROWS
        {
            return Err("stale or mismatched placement");
        }
        let mut previous_index = None;
        let mut previous_y = None;
        for row in &self.rows {
            let Some(metric) = known.iter().find(|metric| metric.uri == row.uri) else {
                return Err("placement row is not cached or measured");
            };
            let Some(index) = request
                .rows
                .iter()
                .position(|candidate| candidate.uri == row.uri)
            else {
                return Err("placement row is not in current viewport");
            };
            if row.body_height != metric.body_height
                || row.body_height > MAX_TEXT_BYTES as u32 * 256
                || row.x > MAX_WIDTH
                || previous_index.is_some_and(|prior| index <= prior)
                || previous_y.is_some_and(|prior| row.y <= prior)
            {
                return Err("invalid placement geometry or ordering");
            }
            previous_index = Some(index);
            previous_y = Some(row.y);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MeasureBudget {
    pub calls: usize,
    pub bytes: usize,
    pub calls_limit: usize,
    pub bytes_limit: usize,
}
impl MeasureBudget {
    pub fn new(calls_limit: usize, bytes_limit: usize) -> Self {
        Self {
            calls: 0,
            bytes: 0,
            calls_limit,
            bytes_limit,
        }
    }
    pub fn charge(&mut self, bytes: usize) -> bool {
        if self.calls >= self.calls_limit || self.bytes.saturating_add(bytes) > self.bytes_limit {
            false
        } else {
            self.calls += 1;
            self.bytes += bytes;
            true
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WrapState {
    source: String,
    cursor: usize,
    prefix_end: usize,
    best: usize,
    line_start: usize,
    lines: Vec<String>,
    continuation: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WrapProgress {
    Pending,
    Complete(Vec<String>, bool),
}

/// Persistent bounded cache for native wrapping work. In-flight states are resumed
/// across frames; completed layouts are retained as shared line arrays.
pub struct WrapCache {
    entries: std::collections::HashMap<(String, u16, u64), CacheEntry>,
    order: std::collections::VecDeque<(String, u16, u64)>,
    bytes: usize,
}
struct CacheEntry {
    source: String,
    state: Option<WrapState>,
    completed: Option<std::sync::Arc<Vec<String>>>,
    continuation: bool,
    accounted: usize,
}
pub const WRAP_CACHE_MAX_ROWS: usize = 200;
pub const WRAP_CACHE_MAX_BYTES: usize = 2 * 1024 * 1024;
impl Default for WrapCache {
    fn default() -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
            bytes: 0,
        }
    }
}
impl WrapCache {
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn has_cached_measure(&self, text: &str, font: u64) -> bool {
        self.entries.iter().any(|((_, _, key_font), entry)| {
            *key_font == font && entry.completed.as_ref().is_some_and(|lines| lines.iter().any(|line| line == text))
        })
    }
    pub fn step<F>(
        &mut self,
        uri: &str,
        source: &str,
        width: u16,
        font: u64,
        budget: &mut MeasureBudget,
        measure: F,
    ) -> Option<(std::sync::Arc<Vec<String>>, bool)>
    where
        F: FnMut(&str) -> Option<u16>,
    {
        let key = (uri.to_owned(), width, font);
        if self
            .entries
            .get(&key)
            .is_some_and(|entry| entry.source != source)
        {
            if let Some(old) = self.entries.remove(&key) {
                self.bytes = self.bytes.saturating_sub(old.accounted);
            }
            self.order.retain(|item| item != &key);
        }
        if !self.entries.contains_key(&key) {
            // The cache key and source snapshot are owned separately from the
            // wrapping state, which owns its sanitized source copy.
            let accounted = uri
                .len()
                .saturating_add(uri.len())
                .saturating_add(source.len().saturating_mul(2));
            if accounted > WRAP_CACHE_MAX_BYTES {
                return None;
            }
            self.entries.insert(
                key.clone(),
                CacheEntry {
                    source: source.to_owned(),
                    state: Some(WrapState::new(source)),
                    completed: None,
                    continuation: false,
                    accounted,
                },
            );
            self.order.push_back(key.clone());
            self.bytes += accounted;
            while self.entries.len() > WRAP_CACHE_MAX_ROWS || self.bytes > WRAP_CACHE_MAX_BYTES {
                if let Some(old_key) = self.order.pop_front() {
                    if let Some(old) = self.entries.remove(&old_key) {
                        self.bytes = self.bytes.saturating_sub(old.accounted);
                    }
                    if old_key == key {
                        return None;
                    }
                } else {
                    break;
                }
            }
        }
        self.order.retain(|item| item != &key);
        self.order.push_back(key.clone());
        let entry = self.entries.get_mut(&key).unwrap();
        if let Some(lines) = &entry.completed {
            return Some((lines.clone(), entry.continuation));
        }
        match entry
            .state
            .as_mut()
            .unwrap()
            .wrap_step(width, budget, measure)
        {
            WrapProgress::Pending => None,
            WrapProgress::Complete(lines, continuation) => {
                let extra: usize = lines
                    .iter()
                    .map(|line| line.len().saturating_add(std::mem::size_of::<String>()))
                    .sum::<usize>()
                    .saturating_add(std::mem::size_of::<Vec<String>>());
                entry.accounted = entry.accounted.saturating_add(extra);
                self.bytes = self.bytes.saturating_add(extra);
                entry.state = None;
                entry.continuation = continuation;
                entry.completed = Some(std::sync::Arc::new(lines));
                let result = entry.completed.as_ref().unwrap().clone();
                while self.entries.len() > WRAP_CACHE_MAX_ROWS || self.bytes > WRAP_CACHE_MAX_BYTES
                {
                    if let Some(old_key) = self.order.pop_front() {
                        if let Some(old) = self.entries.remove(&old_key) {
                            self.bytes = self.bytes.saturating_sub(old.accounted);
                        }
                        if old_key == key {
                            return None;
                        }
                    } else {
                        break;
                    }
                }
                Some((result, continuation))
            }
        }
    }
}
fn extension(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036f | 0x1ab0..=0x1aff | 0x1dc0..=0x1dff | 0x20d0..=0x20ff | 0xfe00..=0xfe0f | 0xfe20..=0xfe2f | 0x1f3fb..=0x1f3ff | 0xe0100..=0xe01ef)
}
impl WrapState {
    pub fn new(source: &str) -> Self {
        let mut clean = String::new();
        for c in source.chars() {
            clean.push(if c == '\0' || (c.is_control() && c != '\n' && c != '\t') {
                '\u{fffd}'
            } else {
                c
            });
        }
        let mut end = clean.len().min(MAX_TEXT_BYTES);
        while !clean.is_char_boundary(end) {
            end -= 1;
        }
        let continuation = clean.len() > end;
        clean.truncate(end);
        Self {
            source: clean,
            continuation,
            ..Self::default()
        }
    }
    pub fn wrap_step<F>(
        &mut self,
        width: u16,
        budget: &mut MeasureBudget,
        mut measure: F,
    ) -> WrapProgress
    where
        F: FnMut(&str) -> Option<u16>,
    {
        loop {
            if self.cursor >= self.source.len() {
                if self.line_start < self.source.len() {
                    self.lines.push(self.source[self.line_start..].to_owned());
                    self.line_start = self.source.len();
                }
                return WrapProgress::Complete(std::mem::take(&mut self.lines), self.continuation);
            }
            let ch = self.source[self.cursor..].chars().next().unwrap();
            if ch == '\n' {
                if self.cursor > self.line_start {
                    self.lines
                        .push(self.source[self.line_start..self.cursor].to_owned());
                } else {
                    self.lines.push(String::new());
                }
                self.cursor += 1;
                self.line_start = self.cursor;
                self.prefix_end = self.cursor;
                self.best = self.cursor;
                continue;
            }
            if self.prefix_end < self.cursor || self.prefix_end - self.line_start >= MAX_LINE_BYTES
            {
                if self.best == self.line_start {
                    self.best = self.prefix_end;
                }
                self.emit_line();
                continue;
            }
            let next = self.prefix_end + ch.len_utf8();
            if !budget.charge(next - self.line_start) {
                return WrapProgress::Pending;
            }
            let Some(measured) = measure(&self.source[self.line_start..next]) else {
                budget.calls -= 1;
                budget.bytes -= next - self.line_start;
                return WrapProgress::Pending;
            };
            self.prefix_end = next;
            if measured <= width {
                self.best = next;
            }
            self.cursor = next;
            if measured > width || self.prefix_end - self.line_start >= MAX_LINE_BYTES {
                if self.best == self.line_start {
                    self.best = next;
                }
                self.emit_line();
            }
        }
    }
    fn emit_line(&mut self) {
        let mut end = self.best.max(self.line_start);
        while end < self.source.len() && end > self.line_start {
            let next = self.source[end..].chars().next().unwrap();
            let prev = self.source[..end].chars().next_back().unwrap();
            if extension(next) || prev == '\u{200d}' {
                end += next.len_utf8();
            } else {
                break;
            }
        }
        if end > self.line_start && end <= self.source.len() {
            self.lines
                .push(self.source[self.line_start..end].trim_end().to_owned());
            self.cursor = end;
            while self.cursor < self.source.len()
                && self.source[self.cursor..]
                    .chars()
                    .next()
                    .unwrap()
                    .is_whitespace()
                && self.source[self.cursor..].chars().next() != Some('\n')
            {
                self.cursor += self.source[self.cursor..]
                    .chars()
                    .next()
                    .unwrap()
                    .len_utf8();
            }
            self.line_start = self.cursor;
            self.prefix_end = self.cursor;
            self.best = self.cursor;
        } else {
            self.continuation = true;
            self.cursor = self.source.len();
        }
    }
}
pub fn wrap_bounded<F>(source: &str, width: u16, mut measure: F) -> (Vec<String>, bool)
where
    F: FnMut(&str) -> u16,
{
    let mut state = WrapState::new(source);
    let mut budget = MeasureBudget::new(MAX_MEASURE_CALLS_ROW, MAX_MEASURE_BYTES_ROW);
    loop {
        match state.wrap_step(width, &mut budget, |s| Some(measure(s))) {
            WrapProgress::Pending => {
                state.continuation = true;
                return (state.lines, state.continuation);
            }
            WrapProgress::Complete(lines, more) => return (lines, more),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn total_measure_call_limit_is_sixty_four() {
        assert_eq!(super::MAX_MEASURE_CALLS_TOTAL, 64);
    }
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
    fn window_validates_contiguous() {
        let h = vec!["a".into(), "b".into(), "c".into()];
        assert!(validate_window(&h, &["b".into(), "c".into()]).is_some());
        assert!(validate_window(&h, &["c".into(), "a".into()]).is_none());
    }
    #[test]
    fn measurement_accepts_valid_partial_prefix_only() {
        let mut r = request();
        r.rows.push(MeasureRow {
            uri: "at://two".into(),
            width: 128,
        });
        let one = MeasureResult {
            uri: "at://one".into(),
            status: MeasureStatus::Ready,
            width: 128,
            line_count: 1,
            line_height: 12,
            baseline_offset: 9,
            body_height: 12,
            continuation: false,
        };
        let partial = MeasureReply {
            schema: 1,
            stamp: r.stamp.clone(),
            rows: vec![one.clone()],
        };
        partial.validate_for(&r).unwrap();
        let mismatch = MeasureReply {
            rows: vec![MeasureResult {
                uri: "at://two".into(),
                ..one
            }],
            ..partial
        };
        assert!(mismatch.validate_for(&r).is_err());
    }
    #[test]
    fn protocol_rejects_stale() {
        let r = request();
        let reply = MeasureReply {
            schema: 1,
            stamp: r.stamp.clone(),
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
        reply.validate_for(&r).unwrap();
    }
    #[test]
    fn wrapping_resumes_after_budget_yield_without_losing_long_word() {
        let src = "x".repeat(600);
        let mut state = WrapState::new(&src);
        let mut budget = MeasureBudget::new(3, 100);
        assert_eq!(
            state.wrap_step(20, &mut budget, |s| Some(s.len() as u16)),
            WrapProgress::Pending
        );
        let output = loop {
            let mut b = MeasureBudget::new(3, 100);
            match state.wrap_step(20, &mut b, |s| Some(s.len() as u16)) {
                WrapProgress::Pending => {}
                WrapProgress::Complete(lines, more) => {
                    assert!(!more);
                    break lines;
                }
            }
        };
        assert!(!output.is_empty());
        assert_eq!(output.concat(), src);
    }
    #[test]
    fn cache_resumes_reuses_and_invalidates_changed_content() {
        let mut cache = WrapCache::default();
        let mut budget = MeasureBudget::new(1, 32);
        assert!(
            cache
                .step("at://a", "hello world", 32, 1, &mut budget, |s| Some(
                    (s.len() * 10) as u16
                ))
                .is_none()
        );
        let mut budget = MeasureBudget::new(100, 4096);
        let first = cache
            .step("at://a", "hello world", 32, 1, &mut budget, |s| {
                Some((s.len() * 10) as u16)
            })
            .unwrap()
            .0;
        let mut budget = MeasureBudget::new(100, 4096);
        let again = cache
            .step("at://a", "hello world", 32, 1, &mut budget, |_| {
                panic!("completed wrapping must be reused")
            })
            .unwrap()
            .0;
        assert!(std::sync::Arc::ptr_eq(&first, &again));
        let mut budget = MeasureBudget::new(100, 4096);
        let changed = cache
            .step("at://a", "changed", 32, 1, &mut budget, |s| {
                Some((s.len() * 10) as u16)
            })
            .unwrap()
            .0;
        assert_ne!(&*first, &*changed);
        cache.clear();
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.bytes(), 0);
    }
    #[test]
    fn long_input_progress_is_bounded_and_cache_is_capped() {
        let mut cache = WrapCache::default();
        let text = "x".repeat(MAX_TEXT_BYTES);
        let mut progress = 0;
        while cache
            .entries
            .get(&("at://long".into(), 32, 1))
            .is_none_or(|e| e.completed.is_none())
        {
            let mut budget = MeasureBudget::new(32, 1024);
            let _ = cache.step("at://long", &text, 32, 1, &mut budget, |s| {
                Some(s.len() as u16)
            });
            assert!(budget.calls <= 32 && budget.bytes <= 1024);
            progress += budget.calls;
            assert!(progress <= MAX_TEXT_BYTES * 32);
        }
        assert!(cache.bytes() <= WRAP_CACHE_MAX_BYTES);
        assert!(cache.len() <= WRAP_CACHE_MAX_ROWS);
    }
    #[test]
    fn yielded_measurement_is_not_sentinel_width() {
        let mut state = WrapState::new("hello");
        let mut b = MeasureBudget::new(10, 100);
        assert_eq!(state.wrap_step(20, &mut b, |_| None), WrapProgress::Pending);
    }
}
