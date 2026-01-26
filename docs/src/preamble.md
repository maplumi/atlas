# The Atlas Runtime Constitution  
*A Spatiotemporal Compute and Rendering Engine*

---

## Preamble

Atlas is founded on the principle that space and time are not visual artifacts but **computational primitives**.  
The engine is not designed to “display maps” but to **compute, simulate, analyze, and render spatiotemporal reality** in a deterministic and scientifically grounded manner.

This Constitution defines the **non-negotiable laws** governing the design, implementation, and evolution of Atlas.  
All subsystems, APIs, formats, and future extensions must comply with these principles.

Any feature that violates these rules is considered invalid, regardless of convenience, popularity, or performance gain.

---

## Article I – Determinism

1. The engine shall be deterministic by default.  
2. Given:
   - the same scene package,
   - the same programs,
   - the same initial state,
   - the same input stream,  
   the engine must always produce identical results.  
3. Randomness, if used, must:
   - be explicitly seeded,
   - be reproducible,
   - be isolatable per subsystem.  
4. Streaming order, decode order, and GPU uploads must not affect final semantic results.

**Rationale:**  
Determinism makes Atlas a scientific instrument, not just a renderer.

---

## Article II – Precision

1. All authoritative spatial data shall exist in **double precision (f64)**.  
2. GPU coordinates shall be **camera-relative** in single precision (f32).  
3. The transformation chain must be exact and explicit:

Geodetic → ECEF → Local Tangent → Camera Space → Clip Space

4. No shortcut projection is permitted if it causes global precision collapse.  
5. Every CRS transformation must be invertible within numerical tolerance.

**Rationale:**  
Visual fidelity without mathematical correctness is illusion.

---

## Article III – Space and Time Are First-Class

1. Every spatial object may optionally possess a temporal extent: 

I = [t_start, t_end]

2. No rendering or analysis may ignore time if time is defined.  
3. All spatial queries are implicitly:

Query = Spatial ∩ Temporal ∩ Visibility ∩ Attribute

4. Time shall never be an afterthought or a “filter”; it is a coordinate.

**Rationale:**  
Atlas is a 4D engine (x, y, z, t).

---

## Article IV – Programmability

1. Cartography, behavior, and analysis are governed by programs.  
2. Programs must be:
- Pure functions  
- Deterministic  
- Side-effect free  
3. Programs shall operate on:

(attributes, time, camera, user state) → (style, visibility, actions)

4. Programs are cacheable and serializable.  
5. Programs must never have direct access to memory, GPU state, or IO.

**Rationale:**  
The engine must be programmable but never unsafe.

---

## Article V – Analysis Is Native

1. Atlas shall never require an external GIS engine for correctness.  
2. All analysis operates on:
- the same spatial indices,
- the same geometry,
- the same temporal structures as rendering.  
3. Analysis results are first-class data:
- selectable,
- renderable,
- programmable,
- storable.

**Rationale:**  
Rendering without analysis is visualization; Atlas is computation.

---

## Article VI – Data Orientation

1. All high-frequency data must be:
- contiguous,
- columnar,
- cache-coherent.  
2. No per-feature heap allocation is permitted in hot paths.  
3. Bitsets and sparse sets are preferred over pointer graphs.  
4. Memory layout is part of the API contract.

**Rationale:**  
Performance emerges from structure, not optimization.

---

## Article VII – Streaming and Residency

1. Every resource has a lifecycle:

Requested → Downloading → Decoding → Building → Uploading → Resident → Evicted

2. No resource may block the render loop.  
3. All streaming must be:
- incremental,
- cancellable,
- budgeted.

**Rationale:**  
The engine must survive slow networks and massive datasets.

---

## Article VIII – Reproducibility

1. Scene packages are immutable scientific artifacts.  
2. A scene package uniquely defines:
- geometry,
- attributes,
- programs,
- temporal ranges,
- rendering behavior.  
3. Hashes define identity.  
4. Versioning is mandatory and forward-compatible.

**Rationale:**  
Atlas scenes are datasets, not deployments.

---

## Article IX – Visibility as Algebra

1. Visibility is not a boolean flag but a logical expression over volumes.  
2. Every visibility rule must be representable as:

V = Boolean Algebra(Volume Tests)

3. The same visibility logic must apply to:
- rendering,
- picking,
- analysis.

**Rationale:**  
What you see must be what you can compute.

---

## Article X – GPU as an Execution Partner, Not a Black Box

1. GPU pipelines must be:
- explicit,
- reproducible,
- debuggable.  
2. Shader behavior is part of the engine contract.  
3. No visual effect may bypass semantic correctness.

**Rationale:**  
The GPU is a co-processor, not a magic brush.
