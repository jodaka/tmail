//! Status-bar spinner for foreground work (plan §11: "Foreground work
//! shows a spinner without freezing input"). Animated from the reducer's
//! tick counter, so snapshots and tests stay deterministic (plan §20: no
//! wall-clock dependence).

/// Braille dot frames: ordinary Unicode, no Nerd Font dependency (plan §18).
const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The spinner frame for the given tick counter.
pub fn frame(ticks: u64) -> &'static str {
    FRAMES[(ticks % FRAMES.len() as u64) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_cycle_deterministically() {
        assert_eq!(frame(0), "⠋");
        assert_eq!(frame(1), "⠙");
        assert_eq!(frame(10), frame(0), "wraps at the frame count");
        assert_eq!(frame(23), frame(3));
    }
}
