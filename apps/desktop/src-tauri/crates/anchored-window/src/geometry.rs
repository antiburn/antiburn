#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Point {
    pub x: f64,
    pub y: f64,
}

pub(crate) fn place_left_preferred(
    anchor: Rect,
    work_area: Rect,
    companion_width_logical: f64,
    companion_height_logical: f64,
    scale_factor: f64,
    gap_logical: f64,
    screen_margin_logical: f64,
) -> Point {
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    let width = companion_width_logical * scale;
    let height = companion_height_logical * scale;
    let gap = gap_logical * scale;
    let margin = screen_margin_logical * scale;
    let left_edge = work_area.x + margin;
    let right_edge = work_area.x + work_area.width - margin;
    let left_x = anchor.x - gap - width;
    let right_x = anchor.x + anchor.width + gap;
    let left_available = (anchor.x - gap - left_edge).max(0.0);
    let right_available = (right_edge - anchor.x - anchor.width - gap).max(0.0);

    let x = if left_x >= left_edge {
        left_x
    } else if right_x + width <= right_edge {
        right_x
    } else if left_available >= right_available {
        left_x.clamp(left_edge, (right_edge - width).max(left_edge))
    } else {
        right_x.clamp(left_edge, (right_edge - width).max(left_edge))
    };
    let y = anchor.y.clamp(
        work_area.y + margin,
        (work_area.y + work_area.height - margin - height).max(work_area.y + margin),
    );

    Point { x, y }
}

#[cfg(test)]
mod tests;
