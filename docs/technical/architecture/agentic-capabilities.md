# Agentic Capabilities

This document outlines the architecture for making Atlas "agentic" - enabling natural language interaction, automated data gathering, analysis, and feedback rendered on the globe.

## Vision

Users should be able to:
- Ask questions in natural language ("Show me areas with high flood risk in Kenya")
- Receive analytical outputs rendered directly on the globe
- Have the system automatically gather, process, and synthesize information
- Get explanations of what the system is doing and why

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                      User Interface                         │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐    │
│  │   Chat   │  │  Voice   │  │ Gestures │  │  Query   │    │
│  │  Input   │  │  Input   │  │          │  │ Builder  │    │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘    │
│       └──────────┬──┴────────────┬┴─────────────┘          │
└──────────────────┼───────────────┼──────────────────────────┘
                   ▼               ▼
┌─────────────────────────────────────────────────────────────┐
│                   Intent Parser                              │
│  - NL → structured query                                     │
│  - Context awareness (current view, selected features)       │
│  - Disambiguation via follow-up                              │
└──────────────────────────────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                   Query Planner                              │
│  - Decompose complex queries                                 │
│  - Identify data sources needed                              │
│  - Plan execution steps                                      │
│  - Estimate cost/time                                        │
└──────────────────────────────────────────────────────────────┘
                           │
          ┌────────────────┼────────────────┐
          ▼                ▼                ▼
┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│ Data Gatherer│  │  Analyzer    │  │ Synthesizer  │
│              │  │              │  │              │
│ - Catalog    │  │ - Compute    │  │ - Summarize  │
│   queries    │  │   graphs     │  │ - Narrate    │
│ - Tile fetch │  │ - Statistics │  │ - Symbolize  │
│ - API calls  │  │ - ML models  │  │ - Annotate   │
└──────┬───────┘  └──────┬───────┘  └──────┬───────┘
       │                 │                 │
       └────────────────┬┴─────────────────┘
                        ▼
┌─────────────────────────────────────────────────────────────┐
│                 Result Renderer                              │
│  - Layer generation                                          │
│  - Animation/transition                                      │
│  - Camera positioning                                        │
│  - Legend/annotation placement                               │
└─────────────────────────────────────────────────────────────┘
                        │
                        ▼
               ┌─────────────────┐
               │     Globe       │
               │   Visualization │
               └─────────────────┘
```

## Module Structure

### Intent Parser (`crates/compute/src/agent/intent.rs`)

Converts natural language to structured queries:

```rust
pub struct Intent {
    pub action: Action,           // Show, Analyze, Compare, Filter, etc.
    pub subject: Subject,         // Features, Layers, Regions
    pub constraints: Vec<Constraint>,
    pub temporal: Option<TimeRange>,
    pub spatial: Option<Bounds>,
}

pub enum Action {
    Show,              // Display data
    Analyze,           // Run analysis
    Compare,           // Side-by-side or temporal comparison
    Filter,            // Subset data
    Explain,           // Describe what's shown
    Navigate,          // Move camera
    Measure,           // Distance, area, etc.
    Export,            // Save/share
}
```

### Query Planner (`crates/compute/src/agent/planner.rs`)

Decomposes intents into executable plans:

```rust
pub struct QueryPlan {
    pub steps: Vec<PlanStep>,
    pub dependencies: HashMap<StepId, Vec<StepId>>,
    pub estimated_duration: Duration,
    pub data_sources: Vec<DataSourceRef>,
}

pub enum PlanStep {
    FetchData { source: DataSourceRef, query: DataQuery },
    ComputeAnalysis { analysis: AnalysisType, inputs: Vec<StepId> },
    GenerateLayer { style: LayerStyle, input: StepId },
    RenderAnnotation { content: AnnotationContent, position: Position },
    AnimateCamera { target: CameraState, duration: Duration },
}
```

### Data Gatherer (`crates/compute/src/agent/gatherer.rs`)

Fetches data from multiple sources:

```rust
pub trait DataGatherer {
    async fn gather(&self, query: &DataQuery) -> Result<Dataset, GatherError>;
    fn sources(&self) -> Vec<DataSourceInfo>;
    fn can_handle(&self, query: &DataQuery) -> bool;
}

// Implementations:
// - CatalogGatherer (STAC, WMS, etc.)
// - TileGatherer (raster/vector tiles)
// - ApiGatherer (external APIs)
// - WebhookGatherer (real-time streams)
```

### Analyzer (`crates/compute/src/agent/analyzer.rs`)

Runs computations on gathered data:

```rust
pub trait Analyzer {
    fn analyze(&self, inputs: Vec<Dataset>) -> AnalysisResult;
    fn capabilities(&self) -> Vec<AnalysisCapability>;
}

pub enum AnalysisCapability {
    Spatial(SpatialAnalysis),    // Buffer, overlay, viewshed
    Statistical(StatAnalysis),    // Mean, std, histogram
    Temporal(TemporalAnalysis),   // Trend, change detection
    MachineLearning(MLAnalysis),  // Classification, clustering
}
```

### Synthesizer (`crates/compute/src/agent/synthesizer.rs`)

Combines results into coherent outputs:

```rust
pub struct Synthesis {
    pub layers: Vec<Layer>,
    pub annotations: Vec<Annotation>,
    pub narrative: String,
    pub suggested_actions: Vec<SuggestedAction>,
}

pub trait Synthesizer {
    fn synthesize(&self, analysis: AnalysisResult, context: &QueryContext) -> Synthesis;
}
```

## Example Flow

**User query:** "Show me areas prone to flooding near Nairobi"

1. **Intent Parser** extracts:
   - Action: Show
   - Subject: Flood-prone areas
   - Spatial: Near Nairobi

2. **Query Planner** creates steps:
   - Fetch elevation data (DEM tiles)
   - Fetch drainage/hydrology data
   - Run flood risk analysis
   - Generate risk layer
   - Position camera over Nairobi

3. **Data Gatherer** fetches:
   - Terrain tiles for Nairobi region
   - OpenStreetMap water features
   - Historical flood data (if available)

4. **Analyzer** computes:
   - Topographic wetness index
   - Distance to water bodies
   - Historical flood frequency

5. **Synthesizer** produces:
   - Choropleth layer (flood risk)
   - Legend (risk levels)
   - Narrative ("Areas in red show highest flood risk...")
   - Suggestions ("Click to see detailed analysis")

6. **Renderer** displays:
   - Fly camera to Nairobi
   - Fade in flood risk layer
   - Show legend and narrative panel

## Integration Points

### With Existing Systems

| System | Integration |
|--------|-------------|
| `streaming::Cache` | Query planner checks cache before fetching |
| `compute::Pipeline` | Analysis runs in compute graph |
| `layers::Layer` | Synthesizer outputs standard layers |
| `scene::Camera` | Renderer controls camera animation |
| Webhook ingestion | Real-time data feeds into gatherer |

### External Services

| Service | Purpose |
|---------|---------|
| LLM API | Intent parsing, narrative generation |
| Geocoding | Location resolution |
| Knowledge graph | Entity linking, context enrichment |
| External APIs | Weather, satellite imagery, etc. |

## Security Considerations

- Rate limiting on agent queries
- Sandbox for external data fetching
- Cost caps for LLM usage
- Audit logging for all agent actions
- User consent for external API calls

## Implementation Phases

### Phase 1: Foundation
- [ ] Intent data structures
- [ ] Basic query planner
- [ ] Integration with existing compute

### Phase 2: Natural Language
- [ ] LLM integration for parsing
- [ ] Context-aware disambiguation
- [ ] Narrative generation

### Phase 3: Smart Gathering
- [ ] Multi-source data fusion
- [ ] Caching and prefetching
- [ ] Quality assessment

### Phase 4: Advanced Analysis
- [ ] ML model integration
- [ ] Temporal analysis
- [ ] Uncertainty quantification

### Phase 5: User Experience
- [ ] Conversational interface
- [ ] Voice input
- [ ] Suggested queries
- [ ] Explanation of reasoning
