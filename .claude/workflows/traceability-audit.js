// Serat traceability audit: every Must/Should/Could requirement has a story, a design element (ADR or architecture.md), and — once code exists — a test citing it.
export const meta = {
  name: 'traceability-audit',
  description: 'Check every FR/NFR in docs/02-requirements/prd.md is covered by a user story, a design element, and a test',
  phases: [{ title: 'Collect' }, { title: 'Audit' }],
}
const LIST = { type: 'object', required: ['ids'], properties: { ids: { type: 'array', items: { type: 'string' } } } }
const reqs = await agent('List every requirement ID matching FR-AAA-nn or NFR-AAA-nn in docs/02-requirements/prd.md whose priority is Must, Should or Could (skip Won\'t).', { label: 'collect:reqs', phase: 'Collect', schema: LIST })
const COVER = { type: 'object', required: ['id', 'stories', 'design', 'tests'], properties: { id: { type: 'string' }, stories: { type: 'array', items: { type: 'string' } }, design: { type: 'array', items: { type: 'string' } }, tests: { type: 'array', items: { type: 'string' } } } }
const coverage = await parallel(reqs.ids.map(id => () =>
  agent(`For requirement ${id}: list the US-nn stories in docs/02-requirements/user-stories.md that serve it; the ADR files or architecture.md sections in docs/03-design that cite it; and any test files under crates/, app/ or bench/ that cite it. Return the paths/ids.`, { label: `audit:${id}`, phase: 'Audit', schema: COVER })))
const gaps = coverage.filter(c => c.stories.length === 0 || c.design.length === 0)
const untested = coverage.filter(c => c.tests.length === 0)
return { total: reqs.ids.length, gaps, untested }
