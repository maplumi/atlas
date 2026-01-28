/// Identifies a streaming request in a deterministic, stable way.
///
/// This is intentionally a small, copyable handle so it can be pushed through
/// the deterministic work queues without heap allocation.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Request(pub u64);
