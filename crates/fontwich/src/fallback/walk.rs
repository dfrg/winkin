//! The lazy family walk for a missed character.

use super::classify;
use super::emoji::Presentation;
use super::key::{self, FallbackKey, FallbackRequest, GenericClass, Han};
use crate::layer::FamilyId;
use crate::{Collection, Family};
use alloc::sync::Arc;

/// Returns the keys a missed character asks, in order, and the Han tradition
/// of the text it is in.
///
/// The keys are the character's own, if it has one, and the Common key.
pub(crate) fn miss_keys(
    collection: &Collection,
    c: char,
    presentation: Presentation,
    request: &FallbackRequest,
) -> ([Option<FallbackKey>; 2], Option<Han>) {
    let (language, generic) = match *request {
        FallbackRequest::Text {
            language, generic, ..
        } => (language, generic),
        FallbackRequest::Generic(_, language) | FallbackRequest::Standard(language) => {
            (language, GenericClass::Plain)
        }
        FallbackRequest::Emoji(_) => (None, GenericClass::Plain),
    };
    let language = language.or(collection.default_language());
    let own = classify::key(c, presentation, language, generic, collection.facts());
    let common = collection.key(&FallbackRequest::Text {
        script: parlance::Script::COMMON,
        language,
        generic,
    });
    let han = key::tradition(parlance::Script::COMMON, language);
    ([own, Some(common)], han)
}

/// The families a missed character walks, lazily, each once.
///
/// The walk goes in stages:
/// 1. the character's own key's families, then the Common key's;
/// 2. the family the platform draws it with (Windows' system fallback);
/// 3. every other primary family of the fallback layers, so no character an
///    installed font maps goes undrawn.
///
/// A control character skips the last stage. Fonts that map one draw it
/// blank.
///
/// Allocates nothing.
pub(crate) struct FallbackFor<'a> {
    collection: &'a Collection,
    c: char,
    /// The Han tradition of the text the character is in, for the
    /// platform's answer.
    han: Option<Han>,
    /// The character's own key, if it has one, and the Common key.
    keys: [Option<FallbackKey>; 2],
    /// Their answers, as the walk reaches them.
    answers: [Option<Arc<[Family]>>; 2],
    stage: Stage,
    key_index: usize,
    key_family_index: usize,
    /// Windows' one system answer, kept to exclude it from the last stage.
    platform_family: Option<Family>,
    layer_index: usize,
    layer_family: FamilyId,
}

/// Where a [`FallbackFor`] walk is.
#[derive(Clone, Copy)]
enum Stage {
    Keys,
    Platform,
    Remaining,
    Done,
}

impl<'a> FallbackFor<'a> {
    pub(crate) fn new(
        collection: &'a Collection,
        c: char,
        presentation: Presentation,
        request: &FallbackRequest,
    ) -> Self {
        let (keys, han) = miss_keys(collection, c, presentation, request);
        Self {
            collection,
            c,
            han,
            keys,
            answers: [None, None],
            stage: Stage::Keys,
            key_index: 0,
            key_family_index: 0,
            platform_family: None,
            layer_index: 0,
            layer_family: FamilyId::FIRST,
        }
    }

    /// Whether `family` is in an answer before the `before`th.
    fn answered(&self, family: &Family, before: usize) -> bool {
        self.answers
            .iter()
            .take(before)
            .flatten()
            .any(|families| families.contains(family))
    }

    /// Returns the family the platform draws the character with, unless a
    /// key already named it.
    ///
    /// Only Windows answers, with one system-wide family whatever the layer.
    fn platform_family(&self) -> Option<Family> {
        for backend in self
            .collection
            .fallback_layers()
            .filter_map(|layer| layer.backend())
        {
            let mut family = None;
            backend.character_family(self.c, self.han, |name| {
                family = self.collection.fallback_family(name);
            });
            if let Some(family) = family {
                return (!self.answered(&family, self.answers.len())).then_some(family);
            }
        }
        None
    }

    fn next_key_family(&mut self) -> Option<Family> {
        while let Some(key) = self.keys.get(self.key_index) {
            let Some(key) = key else {
                self.key_index += 1;
                self.key_family_index = 0;
                continue;
            };
            if self.answers[self.key_index].is_none() {
                self.answers[self.key_index] = Some(self.collection.fallback(key));
            }
            let families = self.answers[self.key_index].as_ref().unwrap();
            match families.get(self.key_family_index) {
                Some(family) => {
                    self.key_family_index += 1;
                    if !self.answered(family, self.key_index) {
                        return Some(family.clone());
                    }
                }
                None => {
                    self.key_index += 1;
                    self.key_family_index = 0;
                }
            }
        }
        None
    }

    fn next_remaining_family(&mut self) -> Option<Family> {
        loop {
            let layer = self.collection.fallback_layers().nth(self.layer_index)?;
            if !layer.holds(self.layer_family) {
                self.layer_index += 1;
                self.layer_family = FamilyId::FIRST;
                continue;
            }
            let id = self.layer_family;
            self.layer_family = id.next();
            if layer.is_secondary(id) {
                continue;
            }
            let family = Family::new(layer, id);
            if self.platform_family.as_ref() != Some(&family)
                && !self.answered(&family, self.answers.len())
            {
                return Some(family);
            }
        }
    }
}

impl Iterator for FallbackFor<'_> {
    type Item = Family;

    fn next(&mut self) -> Option<Family> {
        loop {
            match self.stage {
                Stage::Keys => {
                    if let Some(family) = self.next_key_family() {
                        return Some(family);
                    }
                    self.stage = Stage::Platform;
                }
                Stage::Platform => {
                    self.platform_family = self.platform_family();
                    self.stage = if self.c.is_control() {
                        Stage::Done
                    } else {
                        Stage::Remaining
                    };
                    if let Some(family) = &self.platform_family {
                        return Some(family.clone());
                    }
                }
                Stage::Remaining => {
                    if let Some(family) = self.next_remaining_family() {
                        return Some(family);
                    }
                    self.stage = Stage::Done;
                }
                Stage::Done => return None,
            }
        }
    }
}

impl core::iter::FusedIterator for FallbackFor<'_> {}
