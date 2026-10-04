//! The fonts installed on this computer, for the apps in a Kynoko window.
//!
//! WHY THE LAUNCHER ANSWERS. A page cannot list the fonts of the computer it
//! runs on, on purpose: the list tells computers apart. Chromium offers it
//! behind a permission (`queryLocalFonts()`); WebKit, the engine of a Kynoko
//! window on macOS and Linux, does not offer it at all. An office app that
//! only knows the fonts it ships cannot open a document set in the fonts its
//! author has, so the launcher, which runs on the computer, answers instead.
//! It answers Kynoko's own pages in a Kynoko window only (capability
//! "kynoko-window").
//!
//! WHAT IS GIVEN. First, family names and what each family draws (weights,
//! italic, monospace): enough to fill a font menu and to name a family in
//! CSS, which the engine then finds on the computer by itself
//! (`system_fonts`).
//!
//! Then, for an app that draws text itself (a layout app shapes its lines
//! with HarfBuzz and embeds the glyphs it used in the PDF it exports), the
//! FILE of one installed face, since a name is not enough there. The page
//! names a family, a weight and a style (`system_font_face`), gets back a
//! number that stands for the face during this run of the launcher, and
//! reads the face's file with that number (`system_font_file`). Never a
//! path, never another file: the page cannot name a file, only a face the
//! scan found among the computer's fonts, and what is read must still be a
//! font file of a sane size.
//!
//! The font stays licensed to this computer. The page may draw with it and
//! embed it in a document it exports; honouring the font's embedding
//! permissions when it does (OS/2 `fsType`: installable, editable, preview
//! and print, restricted; no subsetting; bitmap only) is the app's
//! responsibility, as it is any desktop program's that writes a PDF. The
//! launcher cannot check what the page does with the bytes, and does not
//! pretend to.
//!
//! The names are the ones CSS and `queryLocalFonts()` use (the typographic
//! family: "Segoe UI", whose Semibold is a weight, not a family), so that a
//! document made in Chrome and one made in a Kynoko window name their fonts
//! the same way. `system_font_face` takes the same names.

use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The largest font file a page may read. The biggest fonts a computer ships
/// are CJK collections, tens of MB each (35 MB for mingliub.ttc, the biggest
/// of a Windows 11; macOS and Linux have bigger collections of every
/// weight). A font past this is refused with a clear error: the cap keeps a
/// stray file from holding hundreds of MB in the launcher and in the page at
/// once, since the bytes are copied on their way to the page.
const MAX_FONT_FILE: u64 = 128 << 20;

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

/// One face, as `system_font_face` answers it: what the page passes to
/// `system_font_file`, and where the face is in its file.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FontFace {
    /// Stands for the face during this run of the launcher (its place in the
    /// scan). Not a path, and meaningless to another run.
    pub id: u32,
    /// The face's index in its file: non-zero in a `.ttc` / `.otc`
    /// collection, which holds several faces.
    pub index: u32,
}

/// One face, as the scan reads it.
pub(crate) struct Face {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    pub monospace: bool,
    /// The width, 1 (ultra-condensed) to 9 (ultra-expanded), 5 normal.
    pub stretch: u16,
    /// The file the face is in. None for a face held in memory, which has no
    /// file to give.
    pub path: Option<PathBuf>,
    /// Its index in that file (see FontFace).
    pub index: u32,
    /// Its PostScript name, which names it in errors (never its path).
    pub post_script_name: String,
}

impl Face {
    /// Whether a page may be given this face: its family is one a menu
    /// offers (see menu_key), and it is a file on disk.
    fn offered(&self) -> bool {
        self.path.is_some() && menu_key(&self.family).is_some()
    }

    fn label(&self) -> &str {
        if self.post_script_name.is_empty() {
            &self.family
        } else {
            &self.post_script_name
        }
    }
}

/// What the scan keeps: every face, and the families they make.
struct Scan {
    faces: Vec<Face>,
    families: Vec<Family>,
}

static SCAN: OnceLock<Scan> = OnceLock::new();

/// The fonts of this computer, read once per run of the launcher (a font
/// installed meanwhile appears at the next start), for the three commands.
/// Blocking: several hundred files are read the first time.
fn scanned() -> &'static Scan {
    SCAN.get_or_init(scan)
}

/// The families of this computer (`system_fonts`). Blocking, see scanned.
pub fn families() -> &'static [Family] {
    &scanned().families
}

/// The installed face CSS would draw for this family, weight and style
/// (`system_font_face`; see choose). Blocking, see scanned.
pub fn face(name: &str, weight: u16, italic: bool) -> Option<FontFace> {
    choose(&scanned().faces, name, weight, italic)
}

/// The bytes of the file of the face `id` stands for (`system_font_file`).
/// Only for a face `face` could answer: the page never names a path.
/// Blocking.
pub fn file(id: u32) -> Result<Vec<u8>, String> {
    let face = usize::try_from(id)
        .ok()
        .and_then(|i| scanned().faces.get(i))
        .filter(|face| face.offered())
        .ok_or_else(|| format!("no font face {id}: system_font_face gives the number of a face"))?;
    let path = face.path.as_deref().ok_or("this font face has no file")?;
    read_font(path, MAX_FONT_FILE).map_err(|e| format!("the font file of {}: {e}", face.label()))
}

fn scan() -> Scan {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let faces: Vec<Face> = db
        .faces()
        .filter_map(|face| {
            // The first name is the English one when the font has one.
            let (family, _) = face.families.first()?;
            let path = match &face.source {
                fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _) => Some(path.clone()),
                fontdb::Source::Binary(_) => None,
            };
            Some(Face {
                family: family.clone(),
                weight: face.weight.0,
                italic: face.style != fontdb::Style::Normal,
                monospace: face.monospaced,
                stretch: face.stretch.to_number(),
                path,
                index: face.index,
                post_script_name: face.post_script_name.clone(),
            })
        })
        .collect();
    let families = group(&faces);
    Scan { faces, families }
}

/// The key a family is known by: its name without case, when a menu can
/// offer it. None for names a menu cannot offer: empty ones, and the macOS
/// system faces whose name starts with a dot, which the system keeps for
/// itself and CSS cannot name.
fn menu_key(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() || name.starts_with('.') || name.chars().any(char::is_control) {
        return None;
    }
    Some(name.to_lowercase())
}

/// Faces into families: one per name (compared without case, see
/// menu_key), sorted as a person reads them.
pub(crate) fn group(faces: &[Face]) -> Vec<Family> {
    let mut by_key: BTreeMap<String, Family> = BTreeMap::new();
    for face in faces {
        let Some(key) = menu_key(&face.family) else {
            continue;
        };
        let family = by_key.entry(key).or_insert_with(|| Family {
            name: face.family.trim().to_string(),
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

/// Among the faces of the family `name` (the names `system_fonts` gives,
/// compared without case) that are files on disk, the one CSS would draw for
/// this weight and style at the normal width (CSS Fonts 4, "font matching
/// algorithm", in its order):
/// 1. the width: normal first, else the nearest narrower, else the nearest
///    wider;
/// 2. the style: the one asked if the family has it, else the other (italic
///    and oblique count as one, as in `Family`);
/// 3. the weight: the one asked, else, asked 400 to 500, the heavier ones up
///    to 500, then the lighter ones, then those above 500; asked below 400,
///    the lighter ones, then the heavier; asked above 500, the heavier ones,
///    then the lighter. Each time the nearest first.
///
/// Between faces still equal (the same font installed twice), the first one
/// scanned. None when the family has no face to give.
pub(crate) fn choose(faces: &[Face], name: &str, weight: u16, italic: bool) -> Option<FontFace> {
    let key = menu_key(name)?;
    let (id, face) = faces
        .iter()
        .enumerate()
        .filter(|(_, face)| face.offered() && menu_key(&face.family).as_deref() == Some(key.as_str()))
        .min_by_key(|(_, face)| (stretch_rank(face.stretch), face.italic != italic, weight_rank(weight, face.weight)))?;
    Some(FontFace { id: u32::try_from(id).ok()?, index: face.index })
}

/// How far a width is from the normal one, the CSS way (lower is nearer):
/// normal, then narrower, then wider.
fn stretch_rank(stretch: u16) -> (u8, u16) {
    const NORMAL: u16 = 5;
    match stretch {
        NORMAL => (0, 0),
        s if s < NORMAL => (1, NORMAL - s),
        s => (2, s - NORMAL),
    }
}

/// How far a weight is from the one asked, the CSS way (lower is nearer; see
/// choose).
fn weight_rank(asked: u16, weight: u16) -> (u8, u16) {
    let tier = if weight == asked {
        0
    } else if (400..=500).contains(&asked) {
        if weight > asked && weight <= 500 {
            1
        } else if weight < asked {
            2
        } else {
            3
        }
    } else if asked < 400 {
        if weight < asked {
            1
        } else {
            2
        }
    } else if weight > asked {
        1
    } else {
        2
    };
    (tier, asked.abs_diff(weight))
}

/// A font file's bytes, at most `cap` of them, refused when what is there is
/// no longer a font (the file was replaced since the scan read it).
fn read_font(path: &Path, cap: u64) -> Result<Vec<u8>, String> {
    let too_large = |size: u64| format!("{} MiB, more than the {} MiB a font file may be", size >> 20, cap >> 20);
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    if size > cap {
        return Err(too_large(size));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
    // Never more than the cap, even if the file grew since its size was read.
    file.take(cap + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > cap {
        return Err(too_large(bytes.len() as u64));
    }
    if !is_font(&bytes) {
        return Err("not a font file any more".into());
    }
    Ok(bytes)
}

/// Whether the bytes start as the fonts the scan reads do: TrueType or
/// OpenType, alone or in a collection.
fn is_font(bytes: &[u8]) -> bool {
    matches!(bytes.get(..4), Some([0, 1, 0, 0] | b"OTTO" | b"true" | b"ttcf"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A face of a font file of its own, upright, normal width.
    fn sample(family: &str, weight: u16, italic: bool) -> Face {
        Face {
            family: family.into(),
            weight,
            italic,
            monospace: false,
            stretch: 5,
            path: Some(format!("{family}.ttf").into()),
            index: 0,
            post_script_name: String::new(),
        }
    }

    #[test]
    fn faces_become_families() {
        let families = group(&[
            sample("Segoe UI", 400, false),
            sample("Segoe UI", 600, false),
            sample("segoe ui", 400, true),
            sample("Arial", 700, false),
            sample("Arial", 400, false),
            sample(".SF NS", 400, false),
            sample("  ", 400, false),
        ]);
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
        let mono = |family: &str, monospace: bool| Face { monospace, ..sample(family, 400, false) };
        let families = group(&[mono("Consolas", true), mono("Mixed", true), mono("Mixed", false)]);
        assert!(families.iter().find(|f| f.name == "Consolas").unwrap().monospace);
        assert!(!families.iter().find(|f| f.name == "Mixed").unwrap().monospace);
    }

    /// The weight `choose` picks among upright faces of these weights.
    fn chosen_weight(weights: &[u16], asked: u16) -> u16 {
        let faces: Vec<Face> = weights.iter().map(|&w| sample("Family", w, false)).collect();
        let chosen = choose(&faces, "Family", asked, false).unwrap();
        faces[chosen.id as usize].weight
    }

    #[test]
    fn the_nearest_weight_the_css_way() {
        let weights = [100, 300, 500, 600, 900];
        assert_eq!(chosen_weight(&weights, 600), 600);
        // 400 to 500: heavier up to 500 first.
        assert_eq!(chosen_weight(&weights, 400), 500);
        assert_eq!(chosen_weight(&weights, 450), 500);
        // Below 400: lighter first, heavier when there is none.
        assert_eq!(chosen_weight(&weights, 200), 100);
        assert_eq!(chosen_weight(&weights, 350), 300);
        assert_eq!(chosen_weight(&weights, 50), 100);
        // Above 500: heavier first, lighter when there is none.
        assert_eq!(chosen_weight(&weights, 550), 600);
        assert_eq!(chosen_weight(&weights, 700), 900);
        assert_eq!(chosen_weight(&weights, 950), 900);
        // 400 to 500 with nothing up to 500: lighter, then above 500.
        assert_eq!(chosen_weight(&[300, 700], 480), 300);
        assert_eq!(chosen_weight(&[700, 800], 400), 700);
    }

    #[test]
    fn the_style_comes_before_the_weight() {
        let faces = [sample("Serif", 400, false), sample("Serif", 700, true)];
        // The italic asked, even at another weight...
        assert_eq!(choose(&faces, "Serif", 400, true).unwrap().id, 1);
        // ...and the upright one asked, even at another weight.
        assert_eq!(choose(&faces, "Serif", 700, false).unwrap().id, 0);
        // No italic at all: the upright face rather than nothing.
        let upright = [sample("Sans", 400, false), sample("Sans", 700, false)];
        assert_eq!(choose(&upright, "Sans", 700, true).unwrap().id, 1);
    }

    #[test]
    fn the_normal_width_comes_first() {
        let width = |stretch: u16, weight: u16| Face { stretch, ..sample("Din", weight, false) };
        let faces = [width(3, 400), width(5, 700), width(7, 400)];
        assert_eq!(choose(&faces, "Din", 400, false).unwrap().id, 1);
        // No normal width: the narrower before the wider.
        let no_normal = [width(7, 400), width(3, 400)];
        assert_eq!(choose(&no_normal, "Din", 400, false).unwrap().id, 1);
    }

    #[test]
    fn a_collection_keeps_its_index() {
        let in_collection = |family: &str, index: u32| Face { path: Some("msgothic.ttc".into()), index, ..sample(family, 400, false) };
        let faces = [
            sample("Arial", 400, false),
            in_collection("MS Gothic", 0),
            in_collection("MS UI Gothic", 1),
            in_collection("MS PGothic", 2),
        ];
        assert_eq!(choose(&faces, "MS PGothic", 400, false), Some(FontFace { id: 3, index: 2 }));
        assert_eq!(choose(&faces, "ms ui gothic", 400, false), Some(FontFace { id: 2, index: 1 }));
    }

    #[test]
    fn only_a_family_a_menu_offers_and_a_file_on_disk() {
        let in_memory = Face { path: None, ..sample("Memory", 400, false) };
        let faces = [sample("Segoe UI", 400, false), sample(".SF NS", 400, false), in_memory];
        assert_eq!(choose(&faces, "  segoe UI ", 400, false).map(|f| f.id), Some(0));
        assert_eq!(choose(&faces, ".SF NS", 400, false), None);
        assert_eq!(choose(&faces, "Memory", 400, false), None);
        assert_eq!(choose(&faces, "Unknown", 400, false), None);
    }

    #[test]
    fn only_a_font_and_never_past_the_cap() {
        let path = std::env::temp_dir().join(format!("kynoko-launcher-test-{}.ttf", std::process::id()));
        let mut font = b"OTTO".to_vec();
        font.resize(100, 0);
        std::fs::write(&path, &font).unwrap();
        let read = read_font(&path, 1000);
        let capped = read_font(&path, 50);
        std::fs::write(&path, b"#!/bin/sh, not a font").unwrap();
        let not_a_font = read_font(&path, 1000);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(read.unwrap(), font);
        assert!(capped.unwrap_err().contains("more than"));
        assert!(not_a_font.unwrap_err().contains("not a font"));
    }

    #[test]
    fn this_computer_has_fonts() {
        // Every system the launcher runs on ships fonts; a scan that finds
        // none means the scan is broken, not the computer.
        assert!(!families().is_empty());
    }

    #[test]
    fn a_face_of_this_computer_reads() {
        let chosen = families().iter().find_map(|family| face(&family.name, 400, false)).expect("a face on disk");
        assert!(is_font(&file(chosen.id).unwrap()));
        assert!(file(u32::MAX).is_err());
    }
}
