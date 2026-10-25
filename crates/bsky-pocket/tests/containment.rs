use bsky_pocket::{
    adapters::{AllocationBudget, RasterOutput},
    EmbeddedGuest, GuestState,
};

#[test]
fn m0_raster_accounting_reserves_both_buffers_and_releases_on_drop() {
    let budget = AllocationBudget::default();
    let raster = RasterOutput::new_bounded(64, 64, &budget).unwrap();
    assert_eq!(budget.used(), 64 * 64 * 4 * 2);
    let snapshot = raster.pixels().unwrap();
    assert_eq!(snapshot.len(), 64 * 64 * 4);
    assert_eq!(budget.used(), 64 * 64 * 4 * 3);
    drop(raster);
    assert_eq!(budget.used(), 64 * 64 * 4);
    drop(snapshot);
    assert_eq!(budget.used(), 0);
}

#[test]
fn guest_core_can_be_constructed_and_disposed_repeatedly() {
    let mut guest = EmbeddedGuest::new(480, 272).expect("guest initializes");
    assert_eq!(guest.state(), GuestState::Active);
    guest.dispose();
    guest.dispose();
    assert_eq!(guest.state(), GuestState::Disposed);
}
