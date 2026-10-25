//! Deterministic variable-height scroll geometry shared by the embedded timeline host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub uri: String,
    pub height: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub uri: String,
    pub offset: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub first: usize,
    pub last: usize,
    pub y: i64,
}

pub fn window(rows: &[Row], anchor: &Anchor, viewport_height: u32) -> Window {
    let index = rows
        .iter()
        .position(|row| row.uri == anchor.uri)
        .unwrap_or(0);
    let mut y = -(anchor.offset.min(rows.get(index).map_or(0, |r| r.height)) as i64);
    let mut end = index;
    while end < rows.len() && y < viewport_height as i64 {
        y += rows[end].height.max(1) as i64;
        end += 1;
    }
    Window {
        first: index,
        last: end,
        y: -(anchor.offset.min(rows.get(index).map_or(0, |r| r.height)) as i64),
    }
}

pub fn move_anchor(rows: &[Row], anchor: &Anchor, delta: i32, viewport_height: u32) -> Anchor {
    if rows.is_empty() {
        return Anchor {
            uri: String::new(),
            offset: 0,
        };
    }
    let index = rows
        .iter()
        .position(|row| row.uri == anchor.uri)
        .unwrap_or(0);
    let mut absolute = rows[..index]
        .iter()
        .map(|r| r.height.max(1) as i64)
        .sum::<i64>()
        + anchor.offset as i64
        + delta as i64;
    absolute = absolute.max(0);
    let total = rows.iter().map(|r| r.height.max(1) as i64).sum::<i64>();
    absolute = absolute.min(total.saturating_sub(viewport_height as i64).max(0));
    let mut start = 0i64;
    for row in rows {
        let end = start + row.height.max(1) as i64;
        if absolute < end {
            return Anchor {
                uri: row.uri.clone(),
                offset: (absolute - start) as u32,
            };
        }
        start = end;
    }
    Anchor {
        uri: rows.last().unwrap().uri.clone(),
        offset: 0,
    }
}

pub fn reconcile(rows: &[Row], previous: &Anchor, viewport_height: u32) -> Anchor {
    if rows.iter().any(|r| r.uri == previous.uri) {
        let index = rows.iter().position(|r| r.uri == previous.uri).unwrap();
        return Anchor {
            uri: previous.uri.clone(),
            offset: previous.offset.min(rows[index].height.saturating_sub(1)),
        };
    }
    if rows.is_empty() {
        return Anchor {
            uri: String::new(),
            offset: 0,
        };
    }
    let old_index = previous
        .uri
        .strip_prefix("#")
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(0);
    let _ = viewport_height;
    Anchor {
        uri: rows[old_index.min(rows.len() - 1)].uri.clone(),
        offset: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rows() -> Vec<Row> {
        vec![
            Row {
                uri: "a".into(),
                height: 40,
            },
            Row {
                uri: "b".into(),
                height: 300,
            },
            Row {
                uri: "c".into(),
                height: 20,
            },
        ]
    }
    #[test]
    fn tall_rows_scroll_incrementally_and_window_intersects_viewport() {
        let r = rows();
        let a = Anchor {
            uri: "b".into(),
            offset: 0,
        };
        assert_eq!(
            window(&r, &a, 100),
            Window {
                first: 1,
                last: 2,
                y: 0
            }
        );
        assert_eq!(
            move_anchor(&r, &a, 70, 100),
            Anchor {
                uri: "b".into(),
                offset: 70
            }
        );
        assert_eq!(
            move_anchor(&r, &a, -500, 100),
            Anchor {
                uri: "a".into(),
                offset: 0
            }
        );
    }
    #[test]
    fn empty_append_reflow_and_eviction_keep_uri_anchor_when_possible() {
        let a = Anchor {
            uri: "b".into(),
            offset: 12,
        };
        assert_eq!(
            reconcile(&[], &a, 100),
            Anchor {
                uri: String::new(),
                offset: 0
            }
        );
        let mut r = rows();
        r.push(Row {
            uri: "d".into(),
            height: 30,
        });
        assert_eq!(reconcile(&r, &a, 100), a);
        r.remove(0);
        r[0].height = 200;
        assert_eq!(
            reconcile(&r, &a, 100),
            Anchor {
                uri: "b".into(),
                offset: 12
            }
        );
        assert_eq!(
            reconcile(
                &r,
                &Anchor {
                    uri: "a".into(),
                    offset: 0
                },
                100
            )
            .uri,
            "b"
        );
    }
}
