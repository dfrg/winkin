//! Font files on disk, found and read ahead of adding them to a layer.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use std::path::{Path, PathBuf};

use crate::sort;

use crate::font::{
    FileFont, Font, TABLE_LIMIT, family_names, postscript_name, read_u32, standalone_name_table,
};

/// The result of scanning font files.
///
/// [`path`](Self::path) reads names, attributes, and character maps.
/// Outlines remain on disk until [`Font::load`] is called.
///
/// A scan can run on another thread. Use
/// [`LayerBuilder::add_scanned`](crate::LayerBuilder::add_scanned) to add
/// its results to a layer.
#[derive(Debug, Default)]
pub struct Scanned {
    /// Each font, its family's name, and its older family name where it has
    /// another.
    pub(crate) fonts: Vec<(String, Option<String>, Font)>,
}

/// The file extensions of fonts, compared ignoring case.
const EXTENSIONS: [&str; 4] = ["ttf", "otf", "ttc", "otc"];

impl Scanned {
    /// Scans a font file or directory.
    ///
    /// Recursively searches directories for `.ttf`, `.otf`, `.ttc`, and
    /// `.otc` files. Unreadable paths and invalid or unnamed fonts are skipped.
    pub fn path(path: &Path) -> Self {
        let mut scanned = Self::default();
        if path.is_dir() {
            scanned.directory(path);
        } else {
            scanned.file(path);
        }
        scanned
    }

    /// Returns the number of fonts found.
    pub fn len(&self) -> usize {
        self.fonts.len()
    }

    /// Returns `true` if no fonts were found.
    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
    }

    /// Appends the results of another scan.
    pub fn extend(&mut self, other: Self) {
        self.fonts.extend(other.fonts);
    }

    /// Removes older copies of scanned fonts.
    ///
    /// For fonts with the same PostScript name, keeps those with the
    /// highest `head.fontRevision`. Returns the number removed. Fonts
    /// without a PostScript name are neither removed nor used to supersede
    /// others.
    ///
    /// Use after combining scans of system and update directories. This
    /// rereads font files to obtain their names and revisions.
    pub fn drop_superseded(&mut self) -> usize {
        use crate::font::Source;

        // Every font's PostScript name and revision, in scan order.
        let mut identities: Vec<Option<(String, u32)>> = Vec::with_capacity(self.fonts.len());
        let mut table = Vec::new();
        for (_, _, font) in &self.fonts {
            let Source::Path(path) = font.source() else {
                identities.push(None);
                continue;
            };
            identities.push(identity(path, font.index(), &mut table));
        }

        // The highest revision each name reaches, and where it first does.
        let mut best: crate::hash::HashMap<&str, (u32, usize)> = Default::default();
        for (at, identity) in identities.iter().enumerate() {
            let Some((name, revision)) = identity else {
                continue;
            };
            match best.get(name.as_str()) {
                Some((held, _)) if *held >= *revision => {}
                _ => {
                    best.insert(name.as_str(), (*revision, at));
                }
            }
        }
        let keep: Vec<bool> = identities
            .iter()
            .enumerate()
            .map(|(at, identity)| match identity {
                None => true,
                Some((name, _)) => best
                    .get(name.as_str())
                    .is_some_and(|(_, first)| *first == at),
            })
            .collect();

        let dropped = keep.iter().filter(|keep| !**keep).count();
        let mut at = 0;
        self.fonts.retain(|_| {
            at += 1;
            keep[at - 1]
        });
        dropped
    }

    fn directory(&mut self, path: &Path) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        // Sorted, so that a scan adds fonts in the same order every time.
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| Some(entry.ok()?.path()))
            .collect();
        sort::by(&mut paths, Ord::cmp);
        for path in paths {
            if path.is_dir() {
                self.directory(&path);
            } else if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    EXTENSIONS
                        .iter()
                        .any(|known| extension.eq_ignore_ascii_case(known))
                })
            {
                self.file(&path);
            }
        }
    }

    fn file(&mut self, path: &Path) {
        let Some(mut file) = FileFont::opened(path) else {
            return;
        };
        // One path for every font in the file, as a layer shares it.
        let shared: Arc<Path> = Arc::from(path);
        // One open and one `name` buffer for the whole file, however many
        // fonts it holds: a collection can hold dozens.
        let mut table = Vec::new();
        for index in 0..file.fonts() {
            if !file.seat(index) {
                continue;
            }
            let Some((name, older)) = file
                .table_into(b"name", TABLE_LIMIT, &mut table)
                .then(|| {
                    standalone_name_table(&table)
                        .as_ref()
                        .and_then(family_names)
                })
                .flatten()
            else {
                continue;
            };
            let font = Font::from_file(shared.clone(), index, &mut file);
            self.fonts.push((name, older, font));
        }
    }
}

/// A font's PostScript name and `head.fontRevision`, read from its file.
fn identity(path: &Path, index: u32, table: &mut Vec<u8>) -> Option<(String, u32)> {
    let mut file = FileFont::opened(path)?;
    if !file.seat(index) {
        return None;
    }
    let mut name = String::new();
    if !file.table_into(b"name", TABLE_LIMIT, table)
        || !standalone_name_table(table).is_some_and(|table| postscript_name(&table, &mut name))
        || name.is_empty()
    {
        return None;
    }
    // `fontRevision` is the Fixed at offset 4 of `head`.
    let head = file.table(b"head", 8)?;
    let revision = read_u32(&head, 4)?;
    Some((name, revision))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fonts::{cmap, font_collection, font_with_tables, format4, head, name, os2};
    use crate::{LayerBuilder, Role, Source};

    use read_fonts::tables::name::NameId;

    /// A font named `family`, mapping a–z.
    fn font(family: &str) -> Vec<u8> {
        font_with_tables(&[
            (*b"OS/2", os2(400, 5, 0)),
            (*b"cmap", cmap(&[(3, 1, format4(&[(0x61, 0x7A)]))])),
            (*b"name", name(&[(3, 1, NameId::FAMILY_NAME, family)])),
        ])
    }

    /// A font of `family` whose PostScript name is `postscript`, at
    /// `revision`.
    fn versioned(family: &str, postscript: &str, revision: u32) -> Vec<u8> {
        font_with_tables(&[
            (*b"OS/2", os2(400, 5, 0)),
            (*b"head", head(revision)),
            (*b"cmap", cmap(&[(3, 1, format4(&[(0x61, 0x7A)]))])),
            (
                *b"name",
                name(&[
                    (3, 1, NameId::FAMILY_NAME, family),
                    (3, 1, NameId::POSTSCRIPT_NAME, postscript),
                ]),
            ),
        ])
    }

    /// A directory of its own, removed when dropped.
    struct Directory(PathBuf);

    impl Directory {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(alloc::format!(
                "fontwich-scan-{label}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(path.join("sub")).unwrap();
            Self(path)
        }

        fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_directory_scan_skips_invalid_and_unnamed_fonts() {
        let dir = Directory::new("dir");
        dir.write("a.ttf", &font("Alpha"));
        dir.write("sub/b.TTF", &font("Beta"));
        dir.write("c.ttc", &font_collection(&[font("Gamma"), font("Delta")]));
        dir.write("readme.txt", b"not a font, and not looked at");
        dir.write("broken.ttf", b"not a font either");
        dir.write(
            "unnamed.otf",
            &font_with_tables(&[(*b"OS/2", os2(400, 5, 0))]),
        );

        let scanned = Scanned::path(&dir.0);
        assert_eq!(scanned.len(), 4);
        let mut layer = LayerBuilder::new(Role::Application);
        let added = layer.add_scanned(scanned);
        let mut names: Vec<&str> = added.iter().map(|family| family.name()).collect();
        names.sort();
        assert_eq!(names, ["Alpha", "Beta", "Delta", "Gamma"]);
        let gamma = layer.family("gamma").unwrap();
        let font = &gamma.fonts()[0];
        assert!(matches!(font.source(), Source::Path(_)));
        assert!(font.charset().contains('q'));
        // Read only now, and the whole file: the collection, at index 0.
        assert!(font.load().is_some());
        assert_eq!(layer.family("Delta").unwrap().fonts()[0].index(), 1);
    }

    #[test]
    fn a_font_is_found_by_its_older_family_name_too() {
        let dir = Directory::new("older");
        let condensed = font_with_tables(&[
            (*b"OS/2", os2(400, 3, 0)),
            (*b"cmap", cmap(&[(3, 1, format4(&[(0x61, 0x7A)]))])),
            (
                *b"name",
                name(&[
                    (3, 1, NameId::FAMILY_NAME, "Roboto Condensed"),
                    (3, 1, NameId::TYPOGRAPHIC_FAMILY_NAME, "Roboto"),
                ]),
            ),
        ]);
        let path = dir.write("condensed.ttf", &condensed);
        dir.write("regular.ttf", &font("Roboto"));
        let mut layer = LayerBuilder::new(Role::Application);
        let added = layer.add_path(&dir.0);
        assert_eq!(added.len(), 1, "the family, not the older name");
        assert_eq!(layer.family("Roboto").unwrap().fonts().len(), 2);
        let older = layer.family("Roboto Condensed").unwrap();
        assert_eq!(older.fonts().len(), 1);
        assert!(matches!(older.fonts()[0].source(), Source::Path(p) if **p == *path));
        // Another view of fonts "Roboto" has: passed over by coverage.
        let at = layer.layer().find("Roboto Condensed").unwrap();
        assert!(layer.layer().is_secondary(at));
        assert!(
            !layer
                .layer()
                .is_secondary(layer.layer().find("Roboto").unwrap())
        );
    }

    #[test]
    fn a_file_is_added_by_its_path() {
        let dir = Directory::new("file");
        let path = dir.write("a.otf", &font("Alpha"));
        let mut layer = LayerBuilder::new(Role::Application);
        let before = layer.generation();
        let added = layer.add_path(&path);
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].name(), "Alpha");
        assert!(layer.generation() > before);
        // Nothing there: nothing added, and nothing changed.
        let missing = Scanned::path(&dir.0.join("gone.ttf"));
        assert!(missing.is_empty());
        let generation = layer.generation();
        assert!(layer.add_scanned(missing).is_empty());
        assert_eq!(layer.generation(), generation);
    }

    #[test]
    fn the_newer_of_two_copies_of_a_font_supersedes_the_older() {
        // Android's rule, and the reason this exists: the same font in two
        // directories, the platform drawing with the higher revision. Here
        // one directory, since a scan does not care which it came from.
        let directory = Directory::new("superseded");
        directory.write("old.ttf", &versioned("Brand", "Brand-Regular", 0x0001_0000));
        directory.write("new.ttf", &versioned("Brand", "Brand-Regular", 0x0002_0000));
        directory.write(
            "other.ttf",
            &versioned("Other", "Other-Regular", 0x0001_0000),
        );
        // No PostScript name: nothing can supersede it and it supersedes
        // nothing, however many copies of it there are.
        directory.write("anon-1.ttf", &font("Anon"));
        directory.write("anon-2.ttf", &font("Anon"));

        let mut scanned = Scanned::path(directory.0.as_path());
        assert_eq!(scanned.len(), 5);
        assert_eq!(scanned.drop_superseded(), 1);
        assert_eq!(scanned.len(), 4);

        // The one that stayed is the newer file.
        let kept: Vec<&str> = scanned
            .fonts
            .iter()
            .filter(|(family, ..)| family == "Brand")
            .filter_map(|(_, _, font)| match font.source() {
                Source::Path(path) => path.file_name()?.to_str(),
                _ => None,
            })
            .collect();
        assert_eq!(kept, ["new.ttf"]);
        // Dropping again drops nothing.
        assert_eq!(scanned.drop_superseded(), 0);
    }
}
