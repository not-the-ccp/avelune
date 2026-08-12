//! Reusable planar 4:2:0 storage helpers.

/// Layout of a single-allocation 8-bit YUV 4:2:0 frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLayout {
    /// Luma width.
    pub width: usize,
    /// Luma height.
    pub height: usize,
    /// Luma row stride.
    pub y_stride: usize,
    /// Chroma row stride.
    pub c_stride: usize,
    y_len: usize,
    c_len: usize,
}
impl FrameLayout {
    /// Creates a layout with caller-selected row padding multiple.
    pub fn new(width: usize, height: usize, row_multiple: usize) -> Option<Self> {
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return None;
        }
        let m = row_multiple.max(1);
        let align = |v: usize| v.checked_add(m - 1).map(|x| x / m * m);
        let y_stride = align(width)?;
        let c_stride = align(width / 2)?;
        let y_len = y_stride.checked_mul(height)?;
        let c_len = c_stride.checked_mul(height / 2)?;
        y_len.checked_add(c_len.checked_mul(2)?)?;
        Some(Self {
            width,
            height,
            y_stride,
            c_stride,
            y_len,
            c_len,
        })
    }
    /// Total backing allocation length.
    pub const fn allocation_len(self) -> usize {
        self.y_len + self.c_len * 2
    }
    /// Byte offset of the U plane.
    pub const fn u_offset(self) -> usize {
        self.y_len
    }
    /// Byte offset of the V plane.
    pub const fn v_offset(self) -> usize {
        self.y_len + self.c_len
    }
    /// Number of bytes occupied by the luma plane including stride padding.
    pub const fn y_storage_len(self) -> usize {
        self.y_len
    }
    /// Number of bytes occupied by one chroma plane including stride padding.
    pub const fn c_storage_len(self) -> usize {
        self.c_len
    }
    /// Whether all visible rows are tightly packed without padding.
    pub const fn is_tightly_packed(self) -> bool {
        self.y_stride == self.width && self.c_stride == self.width / 2
    }
}

/// Borrowed immutable image plane with explicit visible dimensions and stride.
#[derive(Debug, Clone, Copy)]
pub struct PlaneView<'a> {
    data: &'a [u8],
    /// Visible sample width.
    pub width: usize,
    /// Visible sample height.
    pub height: usize,
    /// Stored row stride in bytes.
    pub stride: usize,
}
impl<'a> PlaneView<'a> {
    /// Complete plane storage including row padding.
    pub const fn storage(self) -> &'a [u8] {
        self.data
    }
    /// One visible row without stride padding.
    pub fn row(self, y: usize) -> Option<&'a [u8]> {
        if y >= self.height {
            return None;
        }
        let start = y.checked_mul(self.stride)?;
        self.data.get(start..start + self.width)
    }
    /// Returns a tightly packed slice when `stride == width`.
    pub fn contiguous(self) -> Option<&'a [u8]> {
        (self.stride == self.width).then_some(&self.data[..self.width * self.height])
    }
}

/// Borrowed mutable image plane with explicit visible dimensions and stride.
#[derive(Debug)]
pub struct PlaneViewMut<'a> {
    data: &'a mut [u8],
    /// Visible sample width.
    pub width: usize,
    /// Visible sample height.
    pub height: usize,
    /// Stored row stride in bytes.
    pub stride: usize,
}
impl<'a> PlaneViewMut<'a> {
    /// Complete mutable plane storage including row padding.
    pub fn storage_mut(&mut self) -> &mut [u8] {
        self.data
    }
    /// One mutable visible row without stride padding.
    pub fn row_mut(&mut self, y: usize) -> Option<&mut [u8]> {
        if y >= self.height {
            return None;
        }
        let start = y.checked_mul(self.stride)?;
        self.data.get_mut(start..start + self.width)
    }
    /// Returns a tightly packed mutable slice when `stride == width`.
    pub fn contiguous_mut(&mut self) -> Option<&mut [u8]> {
        let len = self.width * self.height;
        (self.stride == self.width).then_some(&mut self.data[..len])
    }
}

/// Borrowed immutable 4:2:0 frame view.
#[derive(Debug, Clone, Copy)]
pub struct Frame420View<'a> {
    /// Luma plane.
    pub y: PlaneView<'a>,
    /// Cb/U plane.
    pub u: PlaneView<'a>,
    /// Cr/V plane.
    pub v: PlaneView<'a>,
}

/// Borrowed mutable 4:2:0 frame view.
#[derive(Debug)]
pub struct Frame420ViewMut<'a> {
    /// Luma plane.
    pub y: PlaneViewMut<'a>,
    /// Cb/U plane.
    pub u: PlaneViewMut<'a>,
    /// Cr/V plane.
    pub v: PlaneViewMut<'a>,
}

/// Owned reusable frame allocation with explicit strides and plane offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedFrame420 {
    layout: FrameLayout,
    data: Vec<u8>,
}
impl OwnedFrame420 {
    /// Allocates a zeroed frame.
    pub fn new(layout: FrameLayout) -> Self {
        Self {
            data: vec![0; layout.allocation_len()],
            layout,
        }
    }
    /// Adopts an already tightly packed Y/U/V allocation without copying.
    pub fn from_tightly_packed_data(width: usize, height: usize, data: Vec<u8>) -> Option<Self> {
        let layout = FrameLayout::new(width, height, 1)?;
        if data.len() != layout.allocation_len() {
            return None;
        }
        Some(Self { layout, data })
    }

    /// Builds a tightly packed single-allocation frame from three contiguous planes.
    pub fn from_planes(width: usize, height: usize, y: &[u8], u: &[u8], v: &[u8]) -> Option<Self> {
        let layout = FrameLayout::new(width, height, 1)?;
        if y.len() != width.checked_mul(height)?
            || u.len() != width.checked_mul(height)?.checked_div(4)?
            || v.len() != u.len()
        {
            return None;
        }
        let mut out = Self::new(layout);
        out.y_mut().copy_from_slice(y);
        out.u_mut().copy_from_slice(u);
        out.v_mut().copy_from_slice(v);
        Some(out)
    }
    /// Returns the frame layout.
    pub const fn layout(&self) -> FrameLayout {
        self.layout
    }
    /// Immutable luma storage including stride padding.
    pub fn y(&self) -> &[u8] {
        &self.data[..self.layout.y_len]
    }
    /// Immutable U storage including stride padding.
    pub fn u(&self) -> &[u8] {
        &self.data[self.layout.y_len..self.layout.y_len + self.layout.c_len]
    }
    /// Immutable V storage including stride padding.
    pub fn v(&self) -> &[u8] {
        &self.data[self.layout.y_len + self.layout.c_len..]
    }
    /// Mutable luma storage including stride padding.
    pub fn y_mut(&mut self) -> &mut [u8] {
        &mut self.data[..self.layout.y_len]
    }
    /// Mutable U storage including stride padding.
    pub fn u_mut(&mut self) -> &mut [u8] {
        let start = self.layout.y_len;
        &mut self.data[start..start + self.layout.c_len]
    }
    /// Mutable V storage including stride padding.
    pub fn v_mut(&mut self) -> &mut [u8] {
        let start = self.layout.y_len + self.layout.c_len;
        &mut self.data[start..]
    }
    /// Immutable plane views with visible dimensions separated from stride.
    pub fn view(&self) -> Frame420View<'_> {
        Frame420View {
            y: PlaneView {
                data: self.y(),
                width: self.layout.width,
                height: self.layout.height,
                stride: self.layout.y_stride,
            },
            u: PlaneView {
                data: self.u(),
                width: self.layout.width / 2,
                height: self.layout.height / 2,
                stride: self.layout.c_stride,
            },
            v: PlaneView {
                data: self.v(),
                width: self.layout.width / 2,
                height: self.layout.height / 2,
                stride: self.layout.c_stride,
            },
        }
    }
    /// Mutable plane views with visible dimensions separated from stride.
    pub fn view_mut(&mut self) -> Frame420ViewMut<'_> {
        let y_len = self.layout.y_len;
        let c_len = self.layout.c_len;
        let (y, chroma) = self.data.split_at_mut(y_len);
        let (u, v) = chroma.split_at_mut(c_len);
        Frame420ViewMut {
            y: PlaneViewMut {
                data: y,
                width: self.layout.width,
                height: self.layout.height,
                stride: self.layout.y_stride,
            },
            u: PlaneViewMut {
                data: u,
                width: self.layout.width / 2,
                height: self.layout.height / 2,
                stride: self.layout.c_stride,
            },
            v: PlaneViewMut {
                data: v,
                width: self.layout.width / 2,
                height: self.layout.height / 2,
                stride: self.layout.c_stride,
            },
        }
    }
    /// Mutable backing storage, useful to fill/reuse without reallocating.
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }
    /// Immutable complete backing allocation.
    pub fn data(&self) -> &[u8] {
        &self.data
    }
    /// Zeros the complete allocation while retaining capacity/layout.
    pub fn clear(&mut self) {
        self.data.fill(0);
    }
}

/// Scratch arena reused by stateful codecs.
#[derive(Debug, Default)]
pub struct Scratch {
    bytes: Vec<u8>,
}
impl Scratch {
    /// Returns a zeroed slice of at least `len`, reusing capacity where possible.
    pub fn bytes(&mut self, len: usize) -> &mut [u8] {
        self.bytes.resize(len, 0);
        &mut self.bytes[..len]
    }
    /// Current allocated capacity.
    pub fn capacity(&self) -> usize {
        self.bytes.capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padded_views_preserve_visible_rows_and_offsets() {
        let layout = FrameLayout::new(18, 10, 32).unwrap();
        assert_eq!((layout.y_stride, layout.c_stride), (32, 32));
        let mut frame = OwnedFrame420::new(layout);
        {
            let mut view = frame.view_mut();
            view.y.row_mut(9).unwrap().fill(7);
            view.u.row_mut(4).unwrap().fill(11);
            view.v.row_mut(4).unwrap().fill(13);
        }
        let view = frame.view();
        assert!(view.y.row(9).unwrap().iter().all(|&v| v == 7));
        assert!(view.u.row(4).unwrap().iter().all(|&v| v == 11));
        assert!(view.v.row(4).unwrap().iter().all(|&v| v == 13));
        assert!(view.y.contiguous().is_none());
    }

    #[test]
    fn tightly_packed_planes_share_one_allocation() {
        let y = vec![1; 16 * 8];
        let u = vec![2; 8 * 4];
        let v = vec![3; 8 * 4];
        let frame = OwnedFrame420::from_planes(16, 8, &y, &u, &v).unwrap();
        assert!(frame.layout().is_tightly_packed());
        assert_eq!(frame.data().len(), y.len() + u.len() + v.len());
        assert_eq!(frame.y(), y);
        assert_eq!(frame.u(), u);
        assert_eq!(frame.v(), v);
    }
}
