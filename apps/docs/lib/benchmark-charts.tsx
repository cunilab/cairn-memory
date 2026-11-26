import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import type Benchmark from '../../../bench/after-0.1.9-aliases-v1-shipped.json';

const metrics = [
  ['Hit@5', 'hit_at_5'],
  ['MRR@10', 'mrr_at_10'],
  ['Precision@5', 'precision_at_5'],
] as const;

export function RetrievalChart() {
  const runs = ['v1', 'v2'].map((version) => {
    const file = resolve('..', '..', 'bench', `after-0.1.9-aliases-${version}-shipped.json`);
    const run: typeof Benchmark = JSON.parse(readFileSync(file, 'utf8'));
    return { ...run, version };
  });
  const [v1, v2] = runs;
  return (
    <figure className="benchmark-figure">
      <figcaption>Retrieval quality for Cairn 0.1.9</figcaption>
      <div className="benchmark-axis" aria-hidden="true"><span>0%</span><span>50%</span><span>100%</span></div>
      <dl className="benchmark-metrics">
        {metrics.map(([label, metric]) => (
          <div key={metric}>
            <dt>{label}</dt>
            {runs.map((run) => {
              const value = run.recall.memory_search[metric] * 100;
              return (
                <dd key={run.label} className="benchmark-row">
                  <span>{run.version}</span>
                  <span className="benchmark-track" aria-hidden="true">
                    <span className="benchmark-bar" style={{ width: `${value}%` }} />
                  </span>
                  <span>{value.toFixed(1)}%</span>
                </dd>
              );
            })}
          </div>
        ))}
      </dl>
      <p>v1 used {v1.corpus.questions} questions and no distractors. v2 used {v2.corpus.questions} different questions and {v2.corpus.distractors.toLocaleString('en-US')} distractor memories.</p>
    </figure>
  );
}

export function LatencyChart() {
  // Measured prompt-recall p95 from bench/README.md#automatic-recall-latency.
  const paths = [
    { label: 'Loopback HTTP', ms: 54.3 },
    { label: 'Hook end to end', ms: 442.8 },
  ];
  return (
    <figure className="benchmark-figure">
      <figcaption>Prompt recall latency, p95</figcaption>
      <div className="benchmark-axis latency-axis" aria-hidden="true"><span>0</span><span>250</span><span>500 ms</span></div>
      <dl className="benchmark-metrics">
        {paths.map(({ label, ms }) => (
          <div key={label} className="benchmark-row latency-row">
            <dt>{label}</dt>
            <dd className="benchmark-track" aria-hidden="true">
              <span className="benchmark-bar" style={{ width: `${ms / 5}%` }} />
            </dd>
            <dd>{ms} ms</dd>
          </div>
        ))}
      </dl>
      <p>20 measured requests per recall path on ARM64 macOS with MiniLM and 2,072 stored memories.</p>
    </figure>
  );
}
