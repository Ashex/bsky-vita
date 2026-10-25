use bsky_input::buttons;

/// Convert native Vita buttons to the PocketJS Vita guest button bit layout.
pub fn guest_buttons(native: u32) -> u32 {
    let mut guest = 0;
    for (native_button, guest_button) in [
        (buttons::CROSS, 1 << 14),
        (buttons::CIRCLE, 1 << 13),
        (buttons::TRIANGLE, 1 << 12),
        (buttons::SQUARE, 1 << 15),
        (buttons::UP, 1 << 4),
        (buttons::DOWN, 1 << 6),
        (buttons::LEFT, 1 << 7),
        (buttons::RIGHT, 1 << 5),
    ] {
        if native & native_button != 0 {
            guest |= guest_button;
        }
    }
    guest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_supported_button_to_guest_layout() {
        for (native, guest) in [
            (buttons::CROSS, 1 << 14),
            (buttons::CIRCLE, 1 << 13),
            (buttons::TRIANGLE, 1 << 12),
            (buttons::SQUARE, 1 << 15),
            (buttons::UP, 1 << 4),
            (buttons::DOWN, 1 << 6),
            (buttons::LEFT, 1 << 7),
            (buttons::RIGHT, 1 << 5),
        ] {
            assert_eq!(guest_buttons(native), guest);
        }
        assert_eq!(
            guest_buttons(buttons::TRIANGLE | buttons::CROSS),
            (1 << 12) | (1 << 14)
        );
        assert_eq!(guest_buttons(buttons::START | buttons::SELECT), 0);
    }
}
