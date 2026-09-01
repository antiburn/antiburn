use super::super::{Point, Rect};

pub(super) struct Case {
    pub(super) name: &'static str,
    pub(super) anchor: Rect,
    pub(super) work: Rect,
    pub(super) width: f64,
    pub(super) height: f64,
    pub(super) scale: f64,
    pub(super) expected: Point,
}

pub(super) fn cases() -> [Case; 5] {
    [
        Case {
            name: "left fit",
            anchor: Rect {
                x: 500.0,
                y: 100.0,
                width: 100.0,
                height: 300.0,
            },
            work: Rect {
                x: 0.0,
                y: 0.0,
                width: 1200.0,
                height: 800.0,
            },
            width: 300.0,
            height: 200.0,
            scale: 1.0,
            expected: Point { x: 192.0, y: 100.0 },
        },
        Case {
            name: "right fit",
            anchor: Rect {
                x: 100.0,
                y: 50.0,
                width: 100.0,
                height: 300.0,
            },
            work: Rect {
                x: 0.0,
                y: 0.0,
                width: 900.0,
                height: 700.0,
            },
            width: 300.0,
            height: 200.0,
            scale: 1.0,
            expected: Point { x: 208.0, y: 50.0 },
        },
        Case {
            name: "neither side clamps to roomier side",
            anchor: Rect {
                x: 240.0,
                y: 900.0,
                width: 100.0,
                height: 300.0,
            },
            work: Rect {
                x: 0.0,
                y: 0.0,
                width: 600.0,
                height: 1000.0,
            },
            width: 300.0,
            height: 200.0,
            scale: 1.0,
            expected: Point { x: 292.0, y: 792.0 },
        },
        Case {
            name: "negative monitor origin",
            anchor: Rect {
                x: -500.0,
                y: -850.0,
                width: 100.0,
                height: 300.0,
            },
            work: Rect {
                x: -1200.0,
                y: -900.0,
                width: 1200.0,
                height: 800.0,
            },
            width: 300.0,
            height: 200.0,
            scale: 1.0,
            expected: Point {
                x: -808.0,
                y: -850.0,
            },
        },
        Case {
            name: "logical dimensions convert at monitor scale",
            anchor: Rect {
                x: 1000.0,
                y: 200.0,
                width: 200.0,
                height: 600.0,
            },
            work: Rect {
                x: 0.0,
                y: 0.0,
                width: 2400.0,
                height: 1600.0,
            },
            width: 300.0,
            height: 200.0,
            scale: 2.0,
            expected: Point { x: 384.0, y: 200.0 },
        },
    ]
}
