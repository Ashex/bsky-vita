//! Bounded CPU display and input drivers for the embedded PocketJS runtime.
use pocket_drivers::*;
use std::{cell::RefCell, rc::Rc, vec::Vec};

pub const MAX_RASTER_BYTES: usize = 16 * 1024 * 1024;
pub const M0_ALLOCATION_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Clone, Default)]
pub struct AllocationBudget(Rc<RefCell<usize>>);
impl AllocationBudget {
    pub fn reserve(&self, bytes: usize) -> Result<AllocationReservation, String> {
        let mut used = self.0.borrow_mut();
        let next = used
            .checked_add(bytes)
            .ok_or("M0 allocation size overflow")?;
        if next > M0_ALLOCATION_LIMIT {
            return Err("M0 allocation budget exceeded".into());
        }
        *used = next;
        Ok(AllocationReservation {
            budget: self.clone(),
            bytes,
        })
    }
    pub fn used(&self) -> usize {
        *self.0.borrow()
    }
    fn copy_owned(&self, source: &[u8]) -> Result<OwnedPixels, String> {
        let reservation = self.reserve(source.len())?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(source.len())
            .map_err(|_| "owned buffer allocation failed")?;
        bytes.extend_from_slice(source);
        Ok(OwnedPixels {
            bytes,
            _reservation: reservation,
        })
    }
}
pub struct AllocationReservation {
    budget: AllocationBudget,
    bytes: usize,
}
impl Drop for AllocationReservation {
    fn drop(&mut self) {
        let mut used = self.budget.0.borrow_mut();
        *used = used.saturating_sub(self.bytes);
        self.bytes = 0;
    }
}
pub struct OwnedPixels {
    bytes: Vec<u8>,
    _reservation: AllocationReservation,
}
impl std::ops::Deref for OwnedPixels {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        &self.bytes
    }
}
impl AsRef<[u8]> for OwnedPixels {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
impl std::fmt::Debug for OwnedPixels {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("OwnedPixels")
            .field(&self.bytes.len())
            .finish()
    }
}
impl PartialEq for OwnedPixels {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}
fn zeroed(size: usize) -> Result<Vec<u8>, String> {
    let mut v = Vec::new();
    v.try_reserve_exact(size)
        .map_err(|_| "owned buffer allocation failed")?;
    v.resize(size, 0);
    Ok(v)
}
pub(crate) fn raster_len(width: u32, height: u32) -> Result<usize, String> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .filter(|n| width > 0 && height > 0 && *n <= MAX_RASTER_BYTES)
        .ok_or_else(|| "raster dimensions exceed 16 MiB limit".to_string())
}
pub struct RasterOutput {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    published: Rc<RefCell<Vec<u8>>>,
    budget: AllocationBudget,
    _pixel_reservation: AllocationReservation,
    _published_reservation: Rc<AllocationReservation>,
}
impl RasterOutput {
    pub fn try_clone(&self) -> Result<Self, String> {
        let pixel_reservation = self.budget.reserve(self.pixels.len())?;
        let pixels = zeroed(self.pixels.len())?;
        Ok(Self {
            width: self.width,
            height: self.height,
            pixels,
            published: self.published.clone(),
            budget: self.budget.clone(),
            _pixel_reservation: pixel_reservation,
            _published_reservation: self._published_reservation.clone(),
        })
    }
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        Self::new_bounded(width, height, &AllocationBudget::default())
    }
    pub fn new_bounded(width: u32, height: u32, budget: &AllocationBudget) -> Result<Self, String> {
        let len = raster_len(width, height)?;
        let first = budget.reserve(len)?;
        let second = budget.reserve(len)?;
        let pixels = zeroed(len)?;
        let published = zeroed(len)?;
        Ok(Self {
            width,
            height,
            pixels,
            published: Rc::new(RefCell::new(published)),
            budget: budget.clone(),
            _pixel_reservation: first,
            _published_reservation: Rc::new(second),
        })
    }
    pub fn pixels(&self) -> Result<OwnedPixels, String> {
        self.budget.copy_owned(&self.published.borrow())
    }
}
impl DisplayOutput for RasterOutput {
    type Token = ();
    fn descriptor(&self) -> DisplayDescriptor {
        DisplayDescriptor::new(
            DisplayRole::Primary,
            self.width,
            self.height,
            PixelFormat::Argb8888,
            self.width as usize * 4,
            TargetOwnership::CpuVisible,
        )
    }
    fn acquire(&mut self, region: Rect) -> Result<Target<'_>, DisplayError> {
        if region != Rect::new(0, 0, self.width, self.height) {
            return Err(DisplayError::InvalidTarget);
        }
        let stride = self.width as usize * 4;
        Ok(Target::CpuVisible(Surface::new(
            &mut self.pixels,
            region,
            stride,
            PixelFormat::Argb8888,
        )?))
    }
    fn flush(&mut self, _: &[Rect]) -> Result<(), DisplayError> {
        self.published.borrow_mut().copy_from_slice(&self.pixels);
        Ok(())
    }
    fn wait(&mut self, _: ()) -> Result<(), DisplayError> {
        Ok(())
    }
    fn vsync(&mut self) -> VsyncSource {
        VsyncSource::None
    }
}
pub fn bgra_to_rgba(pixels: &[u8]) -> Result<OwnedPixels, String> {
    bgra_to_rgba_bounded(pixels, &AllocationBudget::default())
}
pub fn bgra_to_rgba_bounded(
    pixels: &[u8],
    budget: &AllocationBudget,
) -> Result<OwnedPixels, String> {
    if pixels.len() % 4 != 0 {
        return Err("BGRA pixel buffer length is not a multiple of four".into());
    }
    let reservation = budget.reserve(pixels.len())?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(pixels.len())
        .map_err(|_| "RGBA conversion allocation failed")?;
    for p in pixels.chunks_exact(4) {
        rgba.extend_from_slice(&[p[2], p[1], p[0], p[3]])
    }
    Ok(OwnedPixels {
        bytes: rgba,
        _reservation: reservation,
    })
}
#[cfg(test)]
mod tests {
    use super::{bgra_to_rgba, AllocationBudget, RasterOutput, M0_ALLOCATION_LIMIT};
    #[test]
    fn converts_known_bgra_pixel_to_rgba() {
        assert_eq!(
            &*bgra_to_rgba(&[0x33, 0x22, 0x11, 0xFF]).unwrap(),
            &[0x11, 0x22, 0x33, 0xFF]
        )
    }
    #[test]
    fn rejects_partial_pixel() {
        assert!(bgra_to_rgba(&[1, 2, 3]).is_err())
    }
    #[test]
    fn reservations_reject_overflow_and_release_exactly_once() {
        let b = AllocationBudget::default();
        let r = b.reserve(M0_ALLOCATION_LIMIT).unwrap();
        assert!(b.reserve(1).is_err());
        drop(r);
        assert_eq!(b.used(), 0)
    }
    #[test]
    fn raster_reservation_rejects_aggregate_over_budget() {
        let b = AllocationBudget::default();
        let _r = b.reserve(M0_ALLOCATION_LIMIT - 1).unwrap();
        assert!(RasterOutput::new_bounded(2048, 2048, &b).is_err());
        assert_eq!(b.used(), M0_ALLOCATION_LIMIT - 1)
    }
    #[test]
    fn raster_clone_reserves_staging_and_shares_published_backing() {
        let b = AllocationBudget::default();
        let r = RasterOutput::new_bounded(2, 2, &b).unwrap();
        let c = r.try_clone().unwrap();
        assert_eq!(b.used(), 48);
        drop(r);
        assert_eq!(b.used(), 32);
        drop(c);
        assert_eq!(b.used(), 0)
    }
    #[test]
    fn snapshots_and_conversion_keep_reservations_for_their_lifetimes() {
        let b = AllocationBudget::default();
        let r = RasterOutput::new_bounded(2, 2, &b).unwrap();
        let s = r.pixels().unwrap();
        assert_eq!(b.used(), 48);
        drop(r);
        assert_eq!(b.used(), 16);
        drop(s);
        assert_eq!(b.used(), 0);
        let c = super::bgra_to_rgba_bounded(&[0; 8], &b).unwrap();
        assert_eq!(b.used(), 8);
        drop(c);
        assert_eq!(b.used(), 0)
    }
    #[test]
    fn fallible_buffer_allocation_rejects_impossible_capacity() {
        assert!(super::zeroed(usize::MAX).is_err())
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TouchContact {
    pub id: u8,
    pub x: u16,
    pub y: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TouchSnapshot {
    pub contacts: [TouchContact; 6],
    pub len: u8,
}
impl TouchSnapshot {
    pub fn from_contacts(contacts: impl IntoIterator<Item = TouchContact>) -> Self {
        let mut snapshot = Self::default();
        for contact in contacts.into_iter().take(snapshot.contacts.len()) {
            snapshot.contacts[snapshot.len as usize] = contact;
            snapshot.len += 1;
        }
        snapshot
    }
    pub fn as_slice(&self) -> &[TouchContact] {
        &self.contacts[..self.len as usize]
    }
}

#[derive(Clone, Default)]
pub struct InputQueue(Rc<RefCell<InputFrameBuilder>>);
impl InputQueue {
    pub fn set_buttons(&self, buttons: u32) {
        let mut f = InputFrameBuilder::new();
        f.buttons = buttons;
        *self.0.borrow_mut() = f
    }
}
impl InputSource for InputQueue {
    fn classes(&self) -> InputClasses {
        InputClasses::BUTTONS
    }
    fn poll(&mut self, out: &mut InputFrameBuilder) {
        *out = std::mem::take(&mut *self.0.borrow_mut())
    }
}

#[cfg(test)]
mod touch_tests {
    use super::{TouchContact, TouchSnapshot};

    #[test]
    fn touch_snapshot_is_fixed_capacity_and_preserves_contact_identity() {
        let snapshot = TouchSnapshot::from_contacts((0..8).map(|id| TouchContact {
            id,
            x: id as u16,
            y: 544,
        }));
        assert_eq!(snapshot.len, 6);
        assert_eq!(snapshot.as_slice()[0].id, 0);
        assert_eq!(snapshot.as_slice()[5].id, 5);
    }
}
