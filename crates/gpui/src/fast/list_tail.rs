//! A `list`'s tail reservation: a viewport's height, less an inset, kept
//! from one row through the last, as a chat transcript keeps the turn just
//! sent at the top of its viewport while the answer streams in below it.
//!
//! The part of the reservation the rows do not fill is trailing space in
//! the last row's box, sized from the rows' natural heights as the list
//! lays them out, before it clamps its scroll and paints. Rows appended
//! below take up the same space, so the content never grows a gap that a
//! scroll could reach between two layouts, as padding written after paint
//! would.
//!
//! Ported from zui (<https://github.com/zeronsh/zui>, `8425850`, "Size
//! transcript tail reservations during list layout"; Apache-2.0, by Wing,
//! wing@anara.com; zui is a fork of Zed's GPUI). Changed from zui: the
//! last row is not rendered again on every layout to learn its natural
//! height. Its height in the list's tree is its natural height plus the
//! extra the list last gave it, so the natural height is read back by
//! taking that extra off, and a scroll layer keeps the row as it keeps any
//! other. The layer counts the extra in its overscan, as part of the last
//! row, and takes it off where it checks a row it renders against the
//! list's height for it ([`natural_height`]). Setting a different
//! reservation is a change of the list's state, which retained views and
//! scroll layers see.

use crate::{Count, ItemLayout, ListItem, ListItemSummary, ListState, Pixels, Size, StateInner};
use collections::VecDeque;
use sum_tree::{Bias, SumTree};

/// What a list keeps of its tail reservation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TailReservation {
    /// The first row the reservation counts, and the inset taken off the
    /// viewport's height.
    reservation: Option<(usize, Pixels)>,
    /// The space added below the last row's natural height, as the list
    /// last laid it out. The last row's height in the list's tree includes
    /// it; no other row's does.
    extra: Pixels,
    /// The extra space the layout under way has added to the last row it
    /// measured so far.
    applied: Pixels,
}

impl ListState {
    /// Reserves one viewport's height, less `inset`, from row `start`
    /// through the last row; `None` removes the reservation.
    ///
    /// What the rows from `start` do not fill is trailing space in the last
    /// row's box. It is worked out from the rows' natural heights as the
    /// list lays them out, before it clamps its scroll and paints, and rows
    /// appended later take it up. Unlike padding set after paint, it never
    /// shows a gap for a frame. It renders no row the list would not render
    /// without it.
    pub fn set_tail_reservation(&self, reservation: Option<(usize, Pixels)>) {
        if self.0.borrow().fast_tail.reservation == reservation {
            return;
        }
        clear_extra(self);
        let state = &mut *self.0.borrow_mut();
        crate::fast::dependencies::StateVersion::bump(&state.version);
        state.fast_tail.reservation = reservation;
    }

    /// Where row `ix` begins, from the top of the list's content: the
    /// heights of the rows before it as the list knows them, measured or
    /// hinted. Leaves the scroll and any pending scroll adjustment as they
    /// are.
    pub fn offset_for_item(&self, ix: usize) -> Pixels {
        let state = self.0.borrow();
        let mut cursor = state.items.cursor::<ListItemSummary>(());
        cursor
            .summary::<_, ListItemSummary>(&Count(ix), Bias::Right)
            .height
    }

    /// Whether the rows from the reservation's first row have, at their
    /// natural heights, filled the reservation: `false` without one. A row
    /// whose height is unknown counts nothing; a hinted height counts as
    /// hinted.
    pub fn tail_reservation_filled(&self) -> bool {
        let state = self.0.borrow();
        let Some((start, inset)) = state.fast_tail.reservation else {
            return false;
        };
        let minimum =
            (state.last_layout_bounds.unwrap_or_default().size.height - inset).max(Pixels::ZERO);
        let mut cursor = state.items.cursor::<ListItemSummary>(());
        let before = cursor
            .summary::<_, ListItemSummary>(&Count(start), Bias::Right)
            .height;
        state.items.summary().height - state.fast_tail.extra - before >= minimum
    }
}

/// Takes the reservation's extra space off the last row of the list of
/// `state`, which is then measured again, before its rows change.
pub(crate) fn clear_extra(state: &ListState) {
    let state = &mut *state.0.borrow_mut();
    if state.fast_tail.extra == Pixels::ZERO || state.items.is_empty() {
        state.fast_tail.extra = Pixels::ZERO;
        return;
    }
    let extra = state.fast_tail.extra;
    let last = state.items.summary().count - 1;
    let mut cursor = state.items.cursor::<Count>(());
    let mut items: SumTree<ListItem> = cursor.slice(&Count(last), Bias::Right);
    if let Some(item) = cursor.item() {
        let size_hint = item.size_hint().map(|mut size| {
            size.height = (size.height - extra).max(Pixels::ZERO);
            size
        });
        items.push(
            ListItem::Unmeasured {
                size_hint,
                focus_handle: item.focus_handle(),
            },
            (),
        );
    }
    drop(cursor);
    state.items = items;
    state.fast_tail.extra = Pixels::ZERO;
}

/// The size of `item`, row `ix` of the list of `state`, as the list last
/// measured it, with the reservation's extra space taken off the last row:
/// the row's natural size, which the list lays out from.
pub(crate) fn natural_size(state: &StateInner, item: &ListItem, ix: usize) -> Option<Size<Pixels>> {
    let mut size = item.size()?;
    if ix + 1 == state.items.summary().count {
        size.height = (size.height - state.fast_tail.extra).max(Pixels::ZERO);
    }
    Some(size)
}

/// The height the list of `state` holds for its row `row`, `height`, less
/// the reservation's extra space if it is the last row: the height the row
/// lays itself out at.
pub(crate) fn natural_height(state: &StateInner, row: usize, height: Pixels) -> Pixels {
    if row + 1 == state.items.summary().count {
        (height - state.fast_tail.extra).max(Pixels::ZERO)
    } else {
        height
    }
}

/// Starts a layout of the rows of the list of `state`: it has added no
/// extra space to the rows it measures yet.
pub(crate) fn begin_layout(state: &mut StateInner) {
    state.fast_tail.applied = Pixels::ZERO;
}

/// Sizes the reservation of the list of `state` as it lays out its rows:
/// `measured` are the rows from `measured_start` at their natural heights
/// (but for the extra space this layout already added), `layouts` the
/// rows it lays out, `old_items` its rows before this layout. If `measured`
/// reaches the last row, the last row is given the extra space the rows
/// from the reservation's first row leave of a viewport of
/// `viewport_height` less the inset. Returns how much that adds to what
/// the rows measured.
pub(crate) fn reserve(
    state: &mut StateInner,
    old_items: &SumTree<ListItem>,
    measured_start: usize,
    measured: &mut VecDeque<ListItem>,
    layouts: &mut VecDeque<ItemLayout>,
    viewport_height: Pixels,
) -> Pixels {
    let applied = state.fast_tail.applied;
    let count = old_items.summary().count;
    if measured.is_empty() || measured_start + measured.len() != count {
        // The last row was not laid out: it keeps the extra it has.
        return Pixels::ZERO;
    }
    let extra = match state.fast_tail.reservation {
        Some((start, inset)) if start < count => {
            let minimum = (viewport_height - inset).max(Pixels::ZERO);
            let mut cursor = old_items.cursor::<ListItemSummary>(());
            cursor.seek(&Count(start), Bias::Right);
            let earlier: ListItemSummary =
                cursor.summary(&Count(measured_start.max(start)), Bias::Right);
            if earlier.has_unknown_height {
                // A jump past rows not measured yet: the list walks back
                // over them first. Counting them as nothing would make a
                // gap.
                Pixels::ZERO
            } else {
                let mut natural = earlier.height;
                for (ix, item) in measured.iter().enumerate() {
                    if measured_start + ix >= start {
                        natural += item.size().map_or(Pixels::ZERO, |size| size.height);
                    }
                }
                natural -= applied;
                (minimum - natural).max(Pixels::ZERO)
            }
        }
        _ => Pixels::ZERO,
    };
    let delta = extra - applied;
    if let Some(ListItem::Measured { size, .. }) = measured.back_mut() {
        size.height += delta;
    }
    if let Some(layout) = layouts
        .back_mut()
        .filter(|layout| layout.index + 1 == count)
    {
        layout.size.height += delta;
    }
    state.fast_tail.applied = extra;
    state.fast_tail.extra = extra;
    delta
}
