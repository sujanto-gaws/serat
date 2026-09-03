// Serat multi-dimension branch review with independent verification.
export const meta = {
  name: 'review-branch',
  description: 'Review a Serat branch for constraint breaches, dependency-rule/ADR violations, budget risk, security and test gaps; verify each finding',
  phases: [{ title: 'Review' }, { title: 'Verify' }],
}
const branch = (args && args.branch) || 'HEAD'
const CTX = 'Read CLAUDE.md, docs/03-design/architecture.md §2 and the ADRs in docs/03-design/adr first. '
const DIMENSIONS = [
  { key: 'constraints', prompt: CTX + `Review git diff main...${branch} for breaches of constraints C-01..C-07: state living only in .serat/, note re-serialisation instead of byte-range patching, JS/UDF in the query path, AI or MCP writes bypassing the review queue.` },
  { key: 'architecture', prompt: CTX + `Review git diff main...${branch} for dependency-rule violations (internal deps other than serat-core; anything depending on serat-app; domain logic in serat-app) and deviations from the ADRs the branch cites.` },
  { key: 'perf', prompt: CTX + `Review git diff main...${branch} for work placed on the webview main thread, shared-mutex index access, and index/parse/query-path changes without a corresponding bench under bench/ (NFR-PRF-01..09).` },
  { key: 'security', prompt: CTX + `Review git diff main...${branch} against docs/03-design/threat-model.md T-01..T-06: extractor code outside the child process or linking HTTP, TCP listeners, key material on disk, telemetry deps, non-Markdown write targets.` },
  { key: 'tests', prompt: CTX + `Review git diff main...${branch} for tests missing or not citing FR/NFR/US IDs, and for patcher/reconciliation/extraction changes lacking property or hostile tests.` },
]
const FINDINGS = { type: 'object', required: ['findings'], properties: { findings: { type: 'array', items: { type: 'object', required: ['title', 'file', 'severity'], properties: { title: { type: 'string' }, file: { type: 'string' }, severity: { type: 'string' }, detail: { type: 'string' }, ids: { type: 'array', items: { type: 'string' } } } } } } }
const VERDICT = { type: 'object', required: ['isReal'], properties: { isReal: { type: 'boolean' }, reason: { type: 'string' } } }
const results = await pipeline(
  DIMENSIONS,
  d => agent(d.prompt, { label: `review:${d.key}`, phase: 'Review', schema: FINDINGS }),
  review => parallel(review.findings.map(f => () =>
    agent(`Adversarially verify against the actual code and the cited ADR/requirement. Finding: ${f.title} in ${f.file}. ${f.detail || ''}`, { label: `verify:${f.file}`, phase: 'Verify', schema: VERDICT }).then(v => ({ ...f, verdict: v })))),
)
return { branch, confirmed: results.flat().filter(Boolean).filter(f => f.verdict && f.verdict.isReal) }
