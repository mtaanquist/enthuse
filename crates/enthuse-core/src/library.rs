//! Arranging libraries (#59), in Settings: favourites are listed first,
//! and the user orders each group by dragging a library or moving it one
//! place at a time.

/// Where the library at `from` lands when dropped at `to`, kept inside
/// its group: favourites stay before the rest, the rest after them.
/// `None` if it ends up where it was.
pub fn drop_target(favorites: &[bool], from: usize, to: usize) -> Option<usize> {
    let group = *favorites.get(from)?;
    let first = favorites.iter().position(|f| *f == group)?;
    let last = favorites.iter().rposition(|f| *f == group)?;
    let target = to.clamp(first, last);
    (target != from).then_some(target)
}

/// The name to save when the user renames a library: trimmed, or the
/// folder's own name when left empty.
pub fn library_name(typed: &str, folder_name: &str) -> String {
    let typed = typed.trim();
    if typed.is_empty() {
        folder_name.to_owned()
    } else {
        typed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_stay_inside_the_group() {
        let favs = [true, true, false, false, false];
        assert_eq!(drop_target(&favs, 0, 1), Some(1));
        assert_eq!(drop_target(&favs, 0, 4), Some(1)); // Past the favourites.
        assert_eq!(drop_target(&favs, 4, 0), Some(2)); // Before the rest.
        assert_eq!(drop_target(&favs, 3, 3), None);
        assert_eq!(drop_target(&favs, 2, 0), None); // Already first of its group.
        assert_eq!(drop_target(&favs, 9, 0), None);
    }

    #[test]
    fn empty_names_fall_back_to_the_folder() {
        assert_eq!(library_name("  Kids' films ", "Movies"), "Kids' films");
        assert_eq!(library_name("   ", "Movies"), "Movies");
    }
}
