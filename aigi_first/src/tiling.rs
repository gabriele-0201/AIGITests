use smithay::{
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point},
};

#[derive(Clone)]
pub enum Split {
    Vertical,
    Horizontal,
}

#[derive(Clone)]
pub struct TilingInfo {
    pub split: Split,
    pub loc: Point<i32, Logical>,
}

impl Default for TilingInfo {
    fn default() -> Self {
        TilingInfo {
            split: Split::Vertical,
            loc: (0, 0).into(),
        }
    }
}

impl TilingInfo {
    pub fn new(split: Split, loc: Point<i32, Logical>) -> Self {
        TilingInfo { split, loc }
    }
}
