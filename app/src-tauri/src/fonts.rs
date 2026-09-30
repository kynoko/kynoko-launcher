//! The fonts installed on this computer, for the apps in a Kynoko window.
//!
//! WHY THE LAUNCHER ANSWERS. A page cannot list the fonts of the computer it
//! runs on, on purpose: the list tells computers apart. Chromium offers it
//! behind a permission (`queryLocalFonts()`); WebKit, the engine of a Kynoko
//! window on macOS and Linux, does not offer it at all. An office app that
//! only knows the fonts it ships cannot open a document set in the fonts its
//! author has, so the launcher, which runs on the computer, answers instead -
//! to Kynoko's own pages in a Kynoko window only (capability "kynoko-window").
//!
//! WHAT IS GIVEN. Family names, and what each family draws (weights, italic,
//! monospace): enough to fill a font menu and to name a family in CSS, which
//! the engine then finds on the computer by itself. Never a font file: a font
//! is licensed to this computer, and nothing here copies it anywhere.
//!
//! The names are the ones CSS and `queryLocalFonts()` use (the typographic
//! family: "Segoe UI", whose Semibold is a weight, not a family), so that a
//! document made in Chrome and one made in a Kynoko window name their fonts
//! the same way.

use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// One family of the computer, as a font menu shows it.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Family {
    pub name: String,
    /// The weights it draws (100 to 900), in order.
    pub weights: Vec<u16>,
    /// Whether it has an italic (or oblique) face.
    pub italic: bool,
    pub monospace: bool,
}

/// One face, as the scan reads it.
pub(crate) struct Face {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    pub monospace: bool,
}

static FAMILIES: OnceLock<Vec<Family>> = OnceLock::new();

/// The families of this computer, read once per run of the launcher (a font
/// installed meanwhile appears at the next start). Blocking: several hundred
/// files are read the first time.
pub fn families() -> &'static [Family] {
    FAMILIES.get_or_init(scan)
}

fn scan() -> Vec<Family> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    group(db.faces().filter_map(|face| {
        // The first name is the English one when the font has one.
        let (family, _) = face.families.first()?;
        Some(Face {
            family: family.clone(),
            weight: face.weight.0,
            italic: face.style != fontdb::Style::Normal,
            monospace: face.monospaced,
        })
    }))
}

/// Faces into families: one per name (compared without case), sorted as a
/// person reads them. Names a menu cannot offer are left out: empty ones, and
/// the macOS system faces whose name starts with a dot, which the system
/// keeps for itself and CSS cannot name.
pub(crate) fn group(faces: impl Iterator<Item = Face>) -> Vec<Family> {
    let mut by_key: BTreeMap<String, Family> = BTreeMap::new();
    for face in faces {
        let name = face.family.trim();
        if name.is_empty() || name.starts_with('.') || name.chars().any(char::is_control) {
            continue;
        }
        let family = by_key.entry(name.to_lowercase()).or_insert_with(|| Family {
            name: name.to_string(),
            weights: Vec::new(),
            italic: false,
            monospace: true,
        });
        if !family.weights.contains(&face.weight) {
            family.weights.push(face.weight);
        }
        family.italic |= face.italic;
        // Monospace only if every face is: a family with one proportional
        // face is not one a code editor can rely on.
        family.monospace &= face.monospace;
    }
    let mut out: Vec<Family> = by_key.into_values().collect();
    for family in &mut out {
        family.weights.sort_unstable();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(family: &str, weight: u16, italic: bool) -> Face {
        Face { family: family.into(), weight, italic, monospace: false }
    }

    #[test]
    fn faces_become_families() {
        let families = group(
            vec![
                face("Segoe UI", 400, false),
                face("Segoe UI", 600, false),
                face("segoe ui", 400, true),
                face("Arial", 700, false),
                face("Arial", 400, false),
                face(".SF NS", 400, false),
                face("  ", 400, false),
            ]
            .into_iter(),
        );
        assert_eq!(
            families,
            vec![
                Family { name: "Arial".into(), weights: vec![400, 700], italic: false, monospace: false },
                Family { name: "Segoe UI".into(), weights: vec![400, 600], italic: true, monospace: false },
            ]
        );
    }

    #[test]
    fn monospace_only_when_every_face_is() {
        let mono = |family: &str, monospace: bool| Face { family: family.into(), weight: 400, italic: false, monospace };
        let families = group(vec![mono("Consolas", true), mono("Mixed", true), mono("Mixed", false)].into_iter());
        assert!(families.iter().find(|f| f.name == "Consolas").unwrap().monospace);
        assert!(!families.iter().find(|f| f.name == "Mixed").unwrap().monospace);
    }

    #[test]
    fn this_computer_has_fonts() {
        // Every system the launcher runs on ships fonts; a scan that finds
        // none means the scan is broken, not the computer.
        assert!(!families().is_empty());
    }
}
