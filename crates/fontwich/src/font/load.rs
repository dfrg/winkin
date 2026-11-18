//! A font file's bytes, read or mapped when asked for and shared by every
//! font naming the file.
//!
//! No state outside the fonts themselves. A layer's fonts share a table of
//! their files, under the lock that guards their shared charsets: each path
//! once, with a [`FontFile`] holding its bytes weakly and an id fixed for its
//! life, made when the layer takes in the first font from the file. A font
//! holds its file's cell, so it knows the id its bytes will have — its
//! [`key`](super::Font::key) — before anything reads them, and loading takes
//! the cell's lock and nothing else. That lock is `std`'s read-write lock:
//! loads of bytes someone still holds share it, and a thread loading a file
//! another thread is reading sleeps until the bytes are in, then shares them.
//! Threads loading other files never wait.
//!
//! Nothing is read until a font is loaded, and most never are. Collections holding the same layer hold the same
//! table, so memory is shared across them with nothing for a caller to hold
//! or pass around; and with the `mmap` feature, separate mappings of one file
//! share the operating system's pages anyway.

use std::path::Path;
use std::sync::{Arc, PoisonError, RwLock, Weak};

use super::FontBytes;

type WeakBytes = Weak<dyn AsRef<[u8]> + Send + Sync>;

/// One font file: its bytes while anyone holds them, and its id.
#[derive(Debug)]
pub(crate) struct FontFile {
    bytes: RwLock<Option<WeakBytes>>,
    /// Fixed when the file is first named, so the bytes keep their id however
    /// many times they are freed and loaded again.
    id: u64,
    /// How many times the file has been read or mapped.
    #[cfg(test)]
    opens: core::sync::atomic::AtomicU32,
}

impl FontFile {
    pub(crate) fn new() -> Self {
        Self {
            bytes: RwLock::new(None),
            id: super::bytes::next_id(),
            #[cfg(test)]
            opens: core::sync::atomic::AtomicU32::new(0),
        }
    }

    /// The id its bytes have whenever they are loaded.
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// The bytes of the file at `path`: those already handed out if someone
    /// still holds them, and otherwise the file, mapped or read.
    pub(crate) fn load(&self, path: &Path) -> Option<FontBytes> {
        // Bytes someone still holds are shared under the read lock, so warm
        // loads of one file from many threads don't wait on each other.
        let held = self
            .bytes
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade);
        if let Some(bytes) = held {
            return Some(FontBytes::from_raw_parts(bytes, self.id));
        }
        // The write lock is held across the read or mapping, so two threads
        // loading one file load it once: the second finds the first's bytes.
        // It is this file's lock alone, and a waiter sleeps. A panic cannot
        // leave the slot half written.
        let mut slot = self.bytes.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(bytes) = slot.as_ref().and_then(Weak::upgrade) {
            return Some(FontBytes::from_raw_parts(bytes, self.id));
        }
        #[cfg(test)]
        self.opens
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let bytes = open(path)?;
        *slot = Some(Arc::downgrade(&bytes));
        Some(FontBytes::from_raw_parts(bytes, self.id))
    }

    /// How many times the file has been read or mapped.
    #[cfg(test)]
    pub(crate) fn opens(&self) -> u32 {
        self.opens.load(core::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(feature = "mmap")]
fn open(path: &Path) -> Option<Arc<dyn AsRef<[u8]> + Send + Sync>> {
    let file = std::fs::File::open(path).ok()?;
    // SAFETY: a mapping is only sound while nobody changes the file under
    // it, and nothing here can promise that for a file on disk. Every font
    // stack that maps fonts makes the same bet — fontique, Skia, FreeType
    // callers — because font files are installed, not edited in place.
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    Some(Arc::new(map))
}

#[cfg(not(feature = "mmap"))]
fn open(path: &Path) -> Option<Arc<dyn AsRef<[u8]> + Send + Sync>> {
    Some(Arc::new(std::fs::read(path).ok()?))
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::sync::Arc;
    use alloc::vec::Vec;

    use crate::test_fonts::{Temporary, font_collection, font_named};
    use crate::{Collection, Font, FontKey, Layer, LayerBuilder, LoadFamily, Role};

    /// `font`'s key, checked against what its bytes load as.
    fn keyed(font: &Font) -> FontKey {
        let key = font.key().expect("a font on disk has a key");
        assert_eq!(Some(key.source), font.load().map(|bytes| bytes.id()));
        assert_eq!(key.index, font.index());
        key
    }

    #[test]
    fn a_font_s_bytes_keep_their_id_across_loads() {
        let file = Temporary::new("kept.ttf", &font_named("Kept"));
        let font = Font::from_path(file.path(), 0).expect("a font");
        let a = font.load().expect("loads");
        let b = font.load().expect("loads");
        assert!(Arc::ptr_eq(a.arc(), b.arc()), "held, so shared");
        assert_eq!(a.data(), std::fs::read(file.path()).expect("readable"));
        let id = a.id();
        drop((a, b));
        // Freed, loaded again, and still the same font to a cache.
        assert_eq!(font.load().expect("loads").id(), id);
    }

    /// Loads `fonts` on as many threads at once, and returns what each got.
    fn load_together(fonts: &[Font]) -> Vec<crate::FontBytes> {
        let barrier = std::sync::Barrier::new(fonts.len());
        std::thread::scope(|scope| {
            let handles: Vec<_> = fonts
                .iter()
                .map(|font| {
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        font.load().expect("loads")
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("no thread panics"))
                .collect()
        })
    }

    #[test]
    fn threads_loading_one_cold_file_read_it_once_and_share_it() {
        let file = Temporary::new("threads.ttf", &font_named("Threads"));
        let font = Font::from_path(file.path(), 0).expect("a font");
        let key = font.key().expect("a font on disk has a key");
        let loaded = load_together(&alloc::vec![font.clone(); 8]);
        for bytes in &loaded {
            assert_eq!(bytes.id(), key.source);
            assert!(Arc::ptr_eq(bytes.arc(), loaded[0].arc()), "one allocation");
        }
        let file = font.file.as_ref().expect("a font on disk has a file");
        assert_eq!(file.opens(), 1);
        drop(loaded);
        // Freed, and loaded again by threads that find it cold once more.
        let again = load_together(&alloc::vec![font.clone(); 4]);
        assert!(again.iter().all(|bytes| bytes.id() == key.source));
        assert!(
            again
                .iter()
                .all(|bytes| Arc::ptr_eq(bytes.arc(), again[0].arc()))
        );
        assert_eq!(file.opens(), 2);
    }

    #[test]
    fn threads_loading_families_sharing_a_cold_file_read_it_once() {
        let file = Temporary::new(
            "threads.ttc",
            &font_collection(&[font_named("One"), font_named("Two")]),
        );
        let mut builder = LayerBuilder::new(Role::Application);
        builder.add_path(file.path());
        let collection = Collection::new().with_layer(builder.snapshot());
        let fonts: Vec<Font> = ["One", "Two"]
            .iter()
            .cycle()
            .take(8)
            .map(|name| collection.family(name).expect("added").fonts()[0].clone())
            .collect();
        let loaded = load_together(&fonts);
        assert!(loaded.iter().all(|bytes| bytes.id() == loaded[0].id()));
        assert!(
            loaded
                .iter()
                .all(|bytes| Arc::ptr_eq(bytes.arc(), loaded[0].arc()))
        );
        let file = fonts[0].file.as_ref().expect("a font on disk has a file");
        assert_eq!(file.opens(), 1);
    }

    #[test]
    fn a_font_s_key_is_known_before_its_bytes_are_loaded() {
        let file = Temporary::new("keyed.ttf", &font_named("Keyed"));
        let font = Font::from_path(file.path(), 0).expect("a font");
        let key = font.key().expect("a font on disk has a key");
        let bytes = font.load().expect("loads");
        assert_eq!(key.source, bytes.id());
        drop(bytes);
        // Freed, loaded again, and still the key it had.
        assert_eq!(keyed(&font), key);
        assert_eq!(font.clone().key(), Some(key));
    }

    #[test]
    fn fonts_sharing_a_file_share_its_key_across_snapshots_and_collections() {
        let file = Temporary::new(
            "keys.ttc",
            &font_collection(&[font_named("One"), font_named("Two")]),
        );
        let mut builder = LayerBuilder::new(Role::Application);
        builder.add_path(file.path());
        let before = builder.snapshot();
        let fonts = |layer: &Arc<Layer>| -> Vec<Font> {
            let collection = Collection::new().with_layer(layer.clone());
            ["One", "Two"]
                .iter()
                .flat_map(|name| collection.family(name).expect("added").fonts().to_vec())
                .collect()
        };
        // Keyed before anything is read, then checked as they load.
        let keys: Vec<FontKey> = fonts(&before).iter().filter_map(Font::key).collect();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].source, keys[1].source, "one file");
        assert_eq!((keys[0].index, keys[1].index), (0, 1));
        let loaded: Vec<FontKey> = fonts(&before).iter().map(keyed).collect();
        assert_eq!(loaded, keys);

        // The same file again, into a later snapshot: new fonts, and the
        // families copied under the one held, all with the file's key.
        builder.add_path(file.path());
        let after = builder.snapshot();
        assert_eq!(fonts(&after).len(), 4);
        for font in fonts(&after) {
            assert_eq!(keyed(&font), keys[font.index() as usize]);
        }
        // A clone of a collection is the same fonts.
        let collection = Collection::new().with_layer(after);
        let one = collection.clone().family("One").expect("added").fonts()[0].key();
        assert_eq!(one, Some(keys[0]));
    }

    #[test]
    fn a_face_over_a_font_on_disk_is_keyed_as_it_loads() {
        // `local()`'s shape: a font found alone, declared as a face.
        let file = Temporary::new("taken.ttf", &font_named("Taken"));
        let alone = Font::from_path(file.path(), 0).expect("a font");
        let mut document = LayerBuilder::new(Role::Document);
        document.add_face_font("Taken", crate::FaceDescriptors::default(), alone.clone());
        let taken = document.family("Taken").expect("declared").fonts()[0].clone();
        // Each keeps to what it loads: the font alone its own file, the face
        // the layer's.
        let (alone, taken) = (keyed(&alone), keyed(&taken));
        assert_eq!(alone.index, taken.index);
    }

    #[test]
    fn a_font_from_bytes_loads_its_own() {
        let font = Font::from_data(font_named("Bytes"), 0);
        let (a, b) = (font.load().expect("loads"), font.load().expect("loads"));
        assert_eq!(a.id(), b.id());
        assert!(Arc::ptr_eq(a.arc(), b.arc()));
    }

    #[test]
    fn a_file_that_is_gone_does_not_load() {
        let file = Temporary::new("gone.ttf", &font_named("Gone"));
        let font = Font::from_path(file.path(), 0).expect("a font");
        drop(file);
        assert!(font.load().is_none());
    }

    /// Two families in one `.ttc`, the shape of Noto Sans CJK's.
    #[derive(Debug)]
    struct OneFile(std::path::PathBuf);

    impl LoadFamily for OneFile {
        fn load(&self, name: &str) -> Vec<Font> {
            let index = if name == "One" { 0 } else { 1 };
            Font::from_path(self.0.clone(), index).into_iter().collect()
        }
    }

    #[test]
    fn families_sharing_a_file_share_its_bytes_and_so_do_collections() {
        let file = Temporary::new(
            "families.ttc",
            &font_collection(&[font_named("One"), font_named("Two")]),
        );
        let layer = Arc::new(Layer::from_names(
            Role::System,
            ["One", "Two"].map(|name| (String::from(name), Vec::new())),
            Arc::new(OneFile(file.path().to_path_buf())),
        ));
        let first = Collection::new().with_layer(layer.clone());
        let second = Collection::new().with_layer(layer);
        let one = first.family("One").expect("listed").fonts()[0]
            .load()
            .expect("loads");
        let two = second.family("Two").expect("listed").fonts()[0]
            .load()
            .expect("loads");
        assert_eq!(one.id(), two.id());
        assert!(Arc::ptr_eq(one.arc(), two.arc()), "one file, loaded once");
        // And one key, told apart by index, the same in both collections.
        let key = |collection: &Collection, name: &str| {
            keyed(&collection.family(name).expect("listed").fonts()[0])
        };
        let (one, two) = (key(&first, "One"), key(&second, "Two"));
        assert_eq!(one.source, two.source);
        assert_eq!((one.index, two.index), (0, 1));
        assert_eq!(key(&second, "One"), one);
    }
}
