//! Mouse hit regions for the rendered frame.

use ratatui::layout::Rect;

/// A navigation entry in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    /// The library view.
    Library,
    /// The queue view.
    Queue,
}

/// A clickable target in the current frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// A sidebar navigation entry.
    Nav(Nav),
    /// A playlist in the sidebar.
    SidebarPlaylist(usize),
    /// A playlist in the library table.
    LibraryRow(usize),
    /// A track in a playlist table.
    TrackRow(usize),
    /// A queue row.
    QueueRow(usize),
    /// A search result row.
    SearchRow(usize),
    /// A context menu row.
    ContextRow(usize),
    /// A playlist in the picker.
    PlaylistPickerRow(usize),
    /// The play and pause button.
    PlayPause,
    /// The next button.
    Next,
    /// The previous button.
    Previous,
    /// The shuffle button.
    Shuffle,
    /// The repeat button.
    Repeat,
    /// The progress bar.
    Progress,
    /// The volume bar.
    Volume,
    /// A toast.
    Toast(usize),
    /// The main pane background.
    MainPane,
    /// The sidebar background.
    SidebarPane,
    /// An overlay background.
    Overlay,
}

/// A collection of hit regions for one frame.
#[derive(Debug, Default)]
pub struct HitMap {
    entries: Vec<(Rect, Hit)>,
}

impl HitMap {
    /// Add a region.
    pub fn push(&mut self, rect: Rect, hit: Hit) {
        if rect.width > 0 && rect.height > 0 {
            self.entries.push((rect, hit));
        }
    }

    /// Return the topmost region at the position.
    pub fn hit(&self, column: u16, row: u16) -> Option<Hit> {
        self.hit_with_rect(column, row).map(|(hit, _)| hit)
    }

    /// Return the topmost region and its rectangle.
    pub fn hit_with_rect(&self, column: u16, row: u16) -> Option<(Hit, Rect)> {
        self.entries
            .iter()
            .rev()
            .find(|(rect, _)| contains(*rect, column, row))
            .map(|(rect, hit)| (*hit, *rect))
    }
}

fn contains(rect: Rect, column: u16, row: u16) -> bool {
    column >= rect.x
        && column < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_topmost_region() {
        let mut map = HitMap::default();
        map.push(Rect::new(0, 0, 10, 10), Hit::Nav(Nav::Library));
        map.push(Rect::new(2, 2, 4, 4), Hit::Nav(Nav::Queue));
        assert_eq!(map.hit(3, 3), Some(Hit::Nav(Nav::Queue)));
        assert_eq!(map.hit(0, 0), Some(Hit::Nav(Nav::Library)));
        assert_eq!(map.hit(20, 20), None);
    }

    #[test]
    fn ignores_empty_regions() {
        let mut map = HitMap::default();
        map.push(Rect::new(0, 0, 0, 5), Hit::PlayPause);
        assert_eq!(map.hit(0, 0), None);
    }
}
