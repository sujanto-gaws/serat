# NFR-PRF-02 rule-selection recordings (task #11, empirical step)

- `record.mjs <arm> <n> <out.json> [jitter]` raw recorder (no rule applied): per frame {ts, run, end, q}, per key {t, pStart, p}, all LoAF and Event Timing (durationThreshold 16) entries.
  Arms: positive, negative (2.2R input-handler block), graded (1.5R), render2/render1/render05 (block of 2.2R/1.0R/0.5R in an rAF callback queued by the input handler, where CM6's measure/DOM update runs), paint (blur/drop-shadow repaint per key).
- `analyze.mjs [--keys] files...` applies every candidate rule offline to the same recordings. `margins.mjs` prints the (ET end - D) distribution.
- `data/summary.txt`, `data/et-margin.txt`: outputs. `data/recordings.tar.gz`: the raw recordings they came from.
- Run: `PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 node record.mjs positive 300 /dev/shm/pos.json`
