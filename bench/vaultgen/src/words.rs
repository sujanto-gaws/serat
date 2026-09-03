//! Vocabulary. Fixed lists, so titles and prose are stable across runs.
//!
//! The words are drawn from the domain a real Serat vault would hold — product
//! work, research, engineering, operations — because note titles feed link
//! text, filenames and full-text search, and lorem ipsum would make FTS ranking
//! behave nothing like it will in production.

/// Subject-matter nouns; the backbone of titles and prose.
pub const DOMAIN_NOUNS: &[&str] = &[
    "pipeline",
    "schema",
    "index",
    "vault",
    "query",
    "latency",
    "throughput",
    "backlog",
    "retrospective",
    "roadmap",
    "migration",
    "rollout",
    "incident",
    "postmortem",
    "budget",
    "contract",
    "interface",
    "protocol",
    "encoding",
    "compaction",
    "checkpoint",
    "snapshot",
    "cache",
    "invalidation",
    "partition",
    "shard",
    "replica",
    "quorum",
    "consensus",
    "ledger",
    "invoice",
    "onboarding",
    "retention",
    "activation",
    "funnel",
    "cohort",
    "segment",
    "forecast",
    "hypothesis",
    "experiment",
    "baseline",
    "regression",
    "telemetry",
    "observability",
    "tracing",
    "profiler",
    "allocator",
    "scheduler",
    "watcher",
    "parser",
    "grammar",
    "typechecker",
    "codegen",
    "renderer",
    "viewport",
    "keystroke",
    "gesture",
    "affordance",
    "taxonomy",
    "ontology",
    "provenance",
    "citation",
    "corpus",
    "embedding",
    "retrieval",
    "ranking",
    "extraction",
    "sandbox",
    "capability",
    "permission",
    "credential",
    "keychain",
    "signature",
    "notarisation",
    "installer",
    "updater",
    "changelog",
    "governance",
    "escalation",
    "handover",
    "runbook",
];

/// Adjectives that qualify a noun in a title.
pub const MODIFIERS: &[&str] = &[
    "incremental",
    "atomic",
    "deterministic",
    "hybrid",
    "local-first",
    "offline",
    "streaming",
    "batched",
    "lazy",
    "eager",
    "structured",
    "typed",
    "untyped",
    "canonical",
    "derived",
    "ephemeral",
    "durable",
    "idempotent",
    "reentrant",
    "sandboxed",
    "signed",
    "audited",
    "quarterly",
    "weekly",
    "legacy",
    "provisional",
    "draft",
    "revised",
    "consolidated",
    "distributed",
    "shared",
    "isolated",
    "nested",
    "flattened",
    "normalised",
    "denormalised",
];

/// Gerunds that open an evergreen note title.
pub const ACTIONS: &[&str] = &[
    "Rethinking",
    "Measuring",
    "Rebuilding",
    "Deprecating",
    "Consolidating",
    "Splitting",
    "Instrumenting",
    "Auditing",
    "Migrating",
    "Benchmarking",
    "Simplifying",
    "Documenting",
    "Automating",
    "Hardening",
    "Unblocking",
    "Scoping",
    "Estimating",
    "Reviewing",
    "Triaging",
];

/// Multi-word ideas, used as whole titles.
pub const CONCEPTS: &[&str] = &[
    "second-order effects",
    "the cost of being wrong",
    "premature abstraction",
    "write amplification",
    "the last mile",
    "conway's law",
    "the bus factor",
    "the happy path",
    "graceful degradation",
    "the principle of least astonishment",
    "irreversible decisions",
    "the tyranny of defaults",
    "sunk cost",
    "the map and the territory",
    "legibility",
    "slack",
    "throughput vs latency",
    "feedback loops",
    "the illusion of control",
    "compounding",
    "the adjacent possible",
    "tacit knowledge",
    "note density",
    "the zettelkasten trap",
    "search as an interface",
    "recall over precision",
    "structure emerges late",
    "naming as design",
    "the shape of data",
];

/// Given names for person notes. ASCII only, so filenames are
/// byte-identical on every filesystem regardless of Unicode normalisation.
pub const FIRST_NAMES: &[&str] = &[
    "Amara", "Bilal", "Chen", "Dagny", "Elif", "Farid", "Greta", "Hana", "Ibrahim", "Juno",
    "Kwame", "Lucia", "Mateo", "Nadia", "Oskar", "Priya", "Quentin", "Rosa", "Sujanto", "Tariq",
    "Ulla", "Viktor", "Wren", "Xiomara", "Yusuf", "Zofia", "Anders", "Beatriz", "Caleb", "Dilnoza",
];

/// Family names for person and literature notes. ASCII only.
pub const LAST_NAMES: &[&str] = &[
    "Achterberg",
    "Baptista",
    "Cordeiro",
    "Duong",
    "Eriksen",
    "Fontaine",
    "Gulyayev",
    "Halvorsen",
    "Iqbal",
    "Jankowski",
    "Kowalczyk",
    "Lindqvist",
    "Mbeki",
    "Nakamura",
    "Okonkwo",
    "Petrosyan",
    "Quiroga",
    "Rasmussen",
    "Salgado",
    "Tjiptoning",
    "Ustinov",
    "Vasquez",
    "Weiss",
    "Xu",
    "Yildirim",
    "Zamora",
];

/// Organisation names for the `org::` field.
pub const ORGS: &[&str] = &[
    "Northwind Labs",
    "Halcyon Systems",
    "Pelagic Data",
    "Meridian Works",
    "Ostara Research",
    "Bramble & Co",
    "Cartographer",
    "Longshore",
    "Vantage Point",
    "Quiet Fields",
];

/// Project codenames, also used for meeting titles.
pub const PROJECT_CODENAMES: &[&str] = &[
    "Lantern",
    "Ferrous",
    "Ostrich",
    "Palisade",
    "Quarry",
    "Rookery",
    "Saltmarsh",
    "Tessellate",
    "Umber",
    "Verdigris",
    "Windrose",
    "Xylem",
    "Yarrow",
    "Zephyr",
    "Anvil",
    "Basalt",
    "Cinder",
    "Dovetail",
    "Ember",
    "Fathom",
    "Granite",
    "Harbour",
    "Ingot",
    "Jetty",
    "Kiln",
    "Lodestone",
];

/// Tags, including nested ones — `topic/performance` must index as a tag.
pub const TAGS: &[&str] = &[
    "reference",
    "permanent",
    "fleeting",
    "literature",
    "project",
    "area",
    "resource",
    "archive",
    "review/weekly",
    "review/monthly",
    "topic/performance",
    "topic/architecture",
    "topic/design",
    "topic/research",
    "topic/ops",
    "topic/writing",
    "status/open",
    "status/blocked",
    "status/done",
    "source/paper",
    "source/talk",
    "source/book",
    "source/thread",
    "meta/inbox",
    "meta/needs-links",
];

/// Values of the `status` field.
pub const STATUSES: &[&str] = &[
    "idea",
    "active",
    "blocked",
    "paused",
    "shipped",
    "abandoned",
];

/// Values of the `priority` field.
pub const PRIORITIES: &[&str] = &["low", "medium", "high", "urgent"];

/// Publication venues for literature notes.
pub const VENUES: &[&str] = &[
    "SIGMOD",
    "OSDI",
    "CHI",
    "VLDB",
    "SOSP",
    "ICSE",
    "USENIX ATC",
    "PLDI",
    "NSDI",
    "FAST",
];

/// First clause of a generated sentence.
pub const SENTENCE_OPENERS: &[&str] = &[
    "The trouble with",
    "What nobody mentions about",
    "A useful framing for",
    "The measured cost of",
    "Everything downstream of",
    "The failure mode of",
    "Two things follow from",
    "The reason",
    "It is worth separating",
    "The interesting part of",
    "What changed my mind about",
    "The cheapest test for",
    "A counterexample to",
    "The boundary case in",
    "Most arguments about",
];

/// Second clause of a generated sentence.
pub const SENTENCE_MIDDLES: &[&str] = &[
    "is that it is invisible until it is expensive",
    "is that the obvious measurement answers a different question",
    "collapses once the vault is large enough to matter",
    "turns out to be a naming problem wearing a performance costume",
    "only holds while the working set fits in page cache",
    "was decided years before anyone wrote it down",
    "depends entirely on whether the index is warm",
    "is not the parsing, it is the fsync",
    "is a property of the graph, not of any one note",
    "stops being true somewhere around ten thousand notes",
    "is reversible, which is why it deserves less debate than it gets",
    "should be tested rather than argued about",
    "is the part that will still be here in five years",
    "is easy to state and hard to hold to",
];

/// A whole trailing sentence.
pub const SENTENCE_CLOSERS: &[&str] = &[
    "The rest is bookkeeping.",
    "Worth revisiting after the benchmark lands.",
    "Not urgent, but it will not get cheaper.",
    "This is the third time it has come up.",
    "See the linked thread for the counterargument.",
    "I no longer believe the first half of this.",
    "Left here deliberately unresolved.",
    "The measurement contradicted the intuition, as usual.",
    "Filed under things I will regret not writing down.",
    "Revisit when the extraction pipeline exists.",
];

/// Imperative verbs that open a task.
pub const TASK_VERBS: &[&str] = &[
    "Draft",
    "Review",
    "Measure",
    "Split",
    "Rewrite",
    "Chase",
    "Confirm",
    "Schedule",
    "Estimate",
    "Reproduce",
    "Document",
    "Delete",
    "Merge",
    "Benchmark",
    "Ask",
    "Decide on",
    "Follow up on",
];

/// Section headings. Also the targets of `[[Note#Heading]]` links, so
/// the body generator and the link generator must agree on this list.
pub const HEADINGS: &[&str] = &[
    "Context",
    "Notes",
    "Open questions",
    "Decision",
    "Evidence",
    "Next actions",
    "References",
    "Log",
    "Summary",
    "Objections",
    "Alternatives considered",
    "What changed",
    "Details",
];

/// Obsidian callout kinds.
pub const CALLOUT_KINDS: &[&str] = &["note", "warning", "tip", "question", "quote", "abstract"];

/// Deterministically title-case the first character of an ASCII-ish string.
pub fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
