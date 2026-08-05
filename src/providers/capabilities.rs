//! Provider/model capability flags.
//!
//! A `Capabilities` value is a bitset describing what a provider (or a
//! single model) can do. The UI uses it to decide which controls to show
//! (e.g. hide the file-attach button when the active provider has no
//! `FILE_UPLOAD`, disable the generate page for providers without image or
//! video generation) without knowing anything about concrete providers.
//!
//! New capabilities can be added by growing the bitmask — the trait
//! interface does not need to change for that.

use std::fmt;
use std::ops::{BitOr, BitOrAssign};

/// A bitset of provider capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Capabilities(u32);

impl Capabilities {
    /// No capabilities at all.
    pub const NONE: Capabilities = Capabilities(0);

    /// Can run text chat conversations.
    pub const CHAT: Capabilities = Capabilities(1 << 0);

    /// Accepts image (multimodal) input in chat.
    pub const VISION: Capabilities = Capabilities(1 << 1);

    /// Can generate images from a prompt.
    pub const IMAGE_GENERATION: Capabilities = Capabilities(1 << 2);

    /// Can generate videos from a prompt or an image.
    pub const VIDEO_GENERATION: Capabilities = Capabilities(1 << 3);

    /// Accepts audio input (e.g. speech-to-text, audio analysis).
    pub const AUDIO_INPUT: Capabilities = Capabilities(1 << 4);

    /// Can synthesize audio output (e.g. text-to-speech, music).
    pub const AUDIO_OUTPUT: Capabilities = Capabilities(1 << 5);

    /// Supports streaming (word-by-word) text responses.
    pub const STREAMING: Capabilities = Capabilities(1 << 6);

    /// Can accept uploaded files (returns a remote handle).
    pub const FILE_UPLOAD: Capabilities = Capabilities(1 << 7);

    /// Exposes an embeddings API.
    pub const EMBEDDINGS: Capabilities = Capabilities(1 << 8);

    /// Create an empty capability set.
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Whether all of `other`'s bits are present in this set.
    pub const fn contains(self, other: Capabilities) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Whether any of `other`'s bits are present in this set.
    pub const fn intersects(self, other: Capabilities) -> bool {
        (self.0 & other.0) != 0
    }

    /// The raw bitmask value.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Add `other`'s bits to this set.
    pub fn insert(&mut self, other: Capabilities) {
        self.0 |= other.0;
    }

    /// Remove `other`'s bits from this set.
    pub fn remove(&mut self, other: Capabilities) {
        self.0 &= !other.0;
    }

    /// Human-readable labels for the set capabilities (for UI badges/tooltips).
    pub fn labels(self) -> Vec<&'static str> {
        const ALL: &[(Capabilities, &str)] = &[
            (Capabilities::CHAT, "Chat"),
            (Capabilities::VISION, "Vision"),
            (Capabilities::IMAGE_GENERATION, "Image generation"),
            (Capabilities::VIDEO_GENERATION, "Video generation"),
            (Capabilities::AUDIO_INPUT, "Audio input"),
            (Capabilities::AUDIO_OUTPUT, "Audio output"),
            (Capabilities::STREAMING, "Streaming"),
            (Capabilities::FILE_UPLOAD, "File upload"),
            (Capabilities::EMBEDDINGS, "Embeddings"),
        ];
        ALL.iter()
            .filter(|(cap, _)| self.contains(*cap))
            .map(|(_, label)| *label)
            .collect()
    }
}

impl BitOr for Capabilities {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for Capabilities {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl fmt::Display for Capabilities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let labels = self.labels();
        write!(f, "{}", labels.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_and_insert() {
        let mut caps = Capabilities::empty();
        assert!(!caps.contains(Capabilities::CHAT));
        caps.insert(Capabilities::CHAT);
        caps.insert(Capabilities::STREAMING);
        assert!(caps.contains(Capabilities::CHAT));
        assert!(caps.contains(Capabilities::CHAT | Capabilities::STREAMING));
        assert!(!caps.contains(Capabilities::VISION));
        assert!(caps.intersects(Capabilities::STREAMING));
    }

    #[test]
    fn bitor_combines() {
        let caps = Capabilities::CHAT | Capabilities::VISION;
        assert!(caps.contains(Capabilities::CHAT));
        assert!(caps.contains(Capabilities::VISION));
    }

    #[test]
    fn labels_are_ordered() {
        let caps = Capabilities::CHAT | Capabilities::VISION | Capabilities::STREAMING;
        assert_eq!(caps.labels(), vec!["Chat", "Vision", "Streaming"]);
    }

    #[test]
    fn display_is_readable() {
        let caps = Capabilities::CHAT | Capabilities::STREAMING;
        assert_eq!(caps.to_string(), "Chat, Streaming");
    }
}
