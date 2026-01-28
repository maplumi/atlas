# Versioning

Versioning rules apply to:
- manifests
- chunks
- programs

Goals:
- backward/forward compatibility
- deterministic decoding
- explicit migrations when required

## Dataset identity

Scene packages have an immutable identity.

Rules:
- `content_hash` is a deterministic hash over the manifest contents (excluding `content_hash` itself).
- When `content_hash` is present, `package_id` must equal `content_hash`.
- Chunk lists are hashed in a canonical order to ensure stability.

Implementation hooks:
- Manifest hashing: `formats::SceneManifest::compute_content_hash_hex`
- Setting identity: `formats::SceneManifest::compute_and_set_identity`
- Validation on load: `formats::ScenePackage::load` rejects mismatched identity.
