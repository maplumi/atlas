/// High-level resource lifecycle states for streaming.
///
/// Target model (see docs/technical/architecture/streaming-and-cache.md):
/// Requested → Downloading → Decoding → Building → Uploading → Resident → Evicted
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ResidencyState {
    Requested,
    Downloading,
    Decoding,
    Building,
    Uploading,
    Resident,
    Evicted,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Residency {
    pub state: ResidencyState,
}

impl Residency {
    pub fn new() -> Self {
        Self {
            state: ResidencyState::Requested,
        }
    }
}

impl Default for Residency {
    fn default() -> Self {
        Self::new()
    }
}
