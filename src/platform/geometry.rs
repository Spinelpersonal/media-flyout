use crate::config::Position;

/// Keep coordinates inside the work area, including negative monitor origins.
pub(super) fn place(
    work: (i32, i32, i32, i32),
    size: (i32, i32),
    margin: i32,
    position: Position,
) -> (i32, i32) {
    let (left, top, right, bottom) = work;
    let free_x = right.saturating_sub(left).saturating_sub(size.0).max(0);
    let free_y = bottom.saturating_sub(top).saturating_sub(size.1).max(0);
    let margin_x = margin.clamp(0, free_x / 2);
    let margin_y = margin.clamp(0, free_y / 2);
    let x = match position {
        Position::TopLeft | Position::BottomLeft => margin_x,
        Position::TopRight | Position::BottomRight => free_x - margin_x,
        _ => free_x / 2,
    };
    let y = match position {
        Position::BottomCenter | Position::BottomLeft | Position::BottomRight => free_y - margin_y,
        _ => margin_y,
    };
    (left.saturating_add(x), top.saturating_add(y))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn respects_taskbar_work_area_and_negative_monitor_origin() {
        assert_eq!(
            place((-1920, 40, 0, 1080), (452, 196), 20, Position::TopCenter),
            (-1186, 60)
        );
        assert_eq!(
            place((-1920, 40, 0, 1080), (452, 196), 20, Position::BottomRight),
            (-472, 864)
        );
    }
    #[test]
    fn clamps_margins_on_small_work_areas() {
        assert_eq!(
            place((0, 0, 500, 220), (452, 196), 200, Position::TopLeft),
            (24, 12)
        );
        assert_eq!(
            place((0, 0, 300, 100), (452, 196), 20, Position::TopCenter),
            (0, 0)
        );
    }
}
