use crate::{
    MAX_NATIVE_COMMAND_BYTES, MAX_NATIVE_COMMANDS, MAX_NATIVE_TEXT_BYTES, NativeBatch,
    NativeCommand,
};

/// Pack adjacent physical lines while keeping each native command within the protocol limits.
pub fn pack_lines(
    base: &[NativeCommand],
    lines: &[(
        f32,
        f32,
        f32,
        u32,
        String,
        Option<crate::visible_text::Stamp>,
        u32,
        u16,
    )],
    image: Option<crate::NativeImage>,
    _line_height: f32,
) -> Result<Vec<NativeCommand>, String> {
    let mut packed: Vec<NativeCommand> = Vec::new();
    for (x, y, scale, color, text, stamp, line, pitch) in lines {
        if text.len() > MAX_NATIVE_TEXT_BYTES {
            return Err("single visible line exceeds native text capacity".into());
        }
        let joins = matches!(packed.last(), Some(NativeCommand::Text { x: px, y: py, scale: ps, color: pc, stamp: pstamp, line: pline, line_height: ppitch, text: previous, .. }) if px == x && *py + f32::from(*ppitch) * line.saturating_sub(*pline) as f32 == *y && *line > *pline && ppitch == pitch && ps == scale && pc == color && pstamp == stamp && previous.len() + 1 + text.len() <= MAX_NATIVE_TEXT_BYTES);
        let mut next = packed.clone();
        if joins {
            if let Some(NativeCommand::Text { text: previous, .. }) = next.last_mut() {
                previous.push('\n');
                previous.push_str(text);
            }
        } else {
            next.push(NativeCommand::Text {
                x: *x,
                y: *y,
                scale: *scale,
                color: *color,
                text: text.clone(),
                stamp: stamp.clone(),
                line: *line,
                line_height: *pitch,
            });
        }
        let mut commands = base.to_vec();
        commands.extend(next.iter().cloned());
        let bytes = NativeBatch {
            commands: commands.clone(),
            image: image.clone(),
        }
        .to_json()?
        .len();
        if commands.len() <= MAX_NATIVE_COMMANDS && bytes <= MAX_NATIVE_COMMAND_BYTES {
            packed = next;
            continue;
        }
        // A line that would overflow a joined chunk starts a fresh command if
        // the remaining command and serialized-byte budgets permit it.
        if joins {
            let mut split = packed.clone();
            split.push(NativeCommand::Text {
                x: *x,
                y: *y,
                scale: *scale,
                color: *color,
                text: text.clone(),
                stamp: stamp.clone(),
                line: *line,
                line_height: *pitch,
            });
            let mut commands = base.to_vec();
            commands.extend(split.iter().cloned());
            let bytes = NativeBatch {
                commands: commands.clone(),
                image: image.clone(),
            }
            .to_json()?
            .len();
            if commands.len() <= MAX_NATIVE_COMMANDS && bytes <= MAX_NATIVE_COMMAND_BYTES {
                packed = split;
                continue;
            }
        }
        return Err("visible text exceeds native batch capacity".into());
    }
    Ok(base.iter().cloned().chain(packed).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn line(
        y: f32,
        n: u32,
        pitch: u16,
        text: &str,
    ) -> (
        f32,
        f32,
        f32,
        u32,
        String,
        Option<crate::visible_text::Stamp>,
        u32,
        u16,
    ) {
        (1.0, y, 1.0, 0xffffffff, text.into(), None, n, pitch)
    }
    #[test]
    fn joins_only_consecutive_geometric_lines_with_matching_pitch() {
        let nine: Vec<_> = (0..9)
            .map(|i| line(4.0 + i as f32 * 9.0, i, 9, "x"))
            .collect();
        let result = pack_lines(&[], &nine, None, 9.0).unwrap();
        assert_eq!(result.len(), 1);
        assert!(
            matches!(&result[0], NativeCommand::Text { text, .. } if text.lines().count() == 9)
        );
        let odd = vec![line(1.0, 0, 9, "a"), line(10.0, 1, 9, "b")];
        assert_eq!(pack_lines(&[], &odd, None, 9.0).unwrap().len(), 1);
        let different = vec![line(1.0, 0, 9, "a"), line(10.0, 1, 10, "b")];
        assert_eq!(pack_lines(&[], &different, None, 9.0).unwrap().len(), 2);
        let gap = vec![line(1.0, 0, 9, "a"), line(20.0, 2, 9, "b")];
        assert_eq!(pack_lines(&[], &gap, None, 9.0).unwrap().len(), 2);
    }
    #[test]
    fn rollover_splits_at_line_boundaries_and_rejects_oversized_line() {
        let rows: Vec<_> = (0..3)
            .map(|i| line(i as f32 * 9.0, i, 9, &"a".repeat(600)))
            .collect();
        let result = pack_lines(&[], &rows, None, 9.0).unwrap();
        assert!(result.len() >= 2);
        assert!(result.iter().all(|command| matches!(command, NativeCommand::Text { text, .. } if text.len() <= MAX_NATIVE_TEXT_BYTES)));
        let oversized = vec![line(0.0, 0, 9, &"x".repeat(MAX_NATIVE_TEXT_BYTES + 1))];
        assert!(pack_lines(&[], &oversized, None, 9.0).is_err());
    }
}
