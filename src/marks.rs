//! Status markers shared by the status row and the log.
//!
//! Every marker must exist in egui's bundled fonts, otherwise it renders as a
//! box. `✘` (U+2718) is not in them, which is why `MISSING` is `✖`.

pub const OK: &str = "✔";
pub const MISSING: &str = "✖";
pub const WARN: &str = "⚠";
pub const PENDING: &str = "…";

#[cfg(test)]
mod tests {
    use super::*;
    use skrifa::{FontRef, MetadataProvider};

    #[test]
    fn markers_exist_in_bundled_fonts() {
        let fonts = [
            epaint_default_fonts::UBUNTU_LIGHT,
            epaint_default_fonts::NOTO_EMOJI_REGULAR,
            epaint_default_fonts::EMOJI_ICON,
        ];
        for c in [OK, MISSING, WARN, PENDING].concat().chars() {
            let covered = fonts
                .iter()
                .any(|bytes| FontRef::new(bytes).unwrap().charmap().map(c).is_some());
            assert!(
                covered,
                "{c} (U+{:04X}) is not in egui's bundled fonts",
                c as u32
            );
        }
    }
}
