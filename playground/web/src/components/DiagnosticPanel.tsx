import { useState } from 'react';

interface AnalysisResult {
  tokens: { json: string | null; error: string | null };
  ast: { json: string | null; error: string | null };
  ir: { json: string | null; error: string | null };
  run: { output: string; error: string | null; exitCode: number };
}

interface TextResult {
  tokens: string;
  ast: string;
  ir: string;
}

type Tab = 'tokens' | 'ast' | 'ir' | 'output';

interface DiagnosticPanelProps {
  analysis: AnalysisResult | null;
  textResult: TextResult | null;
}

export function DiagnosticPanel({ analysis, textResult }: DiagnosticPanelProps) {
  const [activeTab, setActiveTab] = useState<Tab>('output');
  const [format, setFormat] = useState<'text' | 'json'>('text');

  const tabs: { key: Tab; label: string }[] = [
    { key: 'output', label: 'Output' },
    { key: 'tokens', label: 'Tokens' },
    { key: 'ast', label: 'AST' },
    { key: 'ir', label: 'IR' },
  ];

  const showFormatToggle = activeTab !== 'output';

  return (
    <div className="diagnostic-panel">
      <div className="tab-bar">
        {tabs.map(tab => (
          <button
            key={tab.key}
            className={`tab-btn ${activeTab === tab.key ? 'active' : ''}`}
            onClick={() => setActiveTab(tab.key)}
          >
            {tab.label}
          </button>
        ))}
        {showFormatToggle && (
          <div className="format-toggle">
            <button
              className={`format-btn ${format === 'text' ? 'active' : ''}`}
              onClick={() => setFormat('text')}
            >
              Text
            </button>
            <button
              className={`format-btn ${format === 'json' ? 'active' : ''}`}
              onClick={() => setFormat('json')}
            >
              JSON
            </button>
          </div>
        )}
      </div>
      <div className="tab-content">
        {!analysis ? (
          <div className="diagnostic-empty">Enter some Petal code to see results</div>
        ) : activeTab === 'output' ? (
          <OutputView run={analysis.run} />
        ) : activeTab === 'tokens' ? (
          <TokenView
            json={analysis.tokens.json}
            text={textResult?.tokens ?? null}
            error={analysis.tokens.error}
            format={format}
          />
        ) : activeTab === 'ast' ? (
          <JsonTextView
            json={analysis.ast.json}
            text={textResult?.ast ?? null}
            error={analysis.ast.error}
            format={format}
          />
        ) : (
          <IrView
            json={analysis.ir.json}
            text={textResult?.ir ?? null}
            error={analysis.ir.error}
            format={format}
          />
        )}
      </div>
    </div>
  );
}

function OutputView({ run }: { run: AnalysisResult['run'] }) {
  return (
    <div className="run-output">
      {run.output && <div className="run-stdout">{run.output}</div>}
      {run.error && <div className="run-stderr">{run.error}</div>}
      {!run.output && !run.error && (
        <div className="diagnostic-empty">No output</div>
      )}
      {(run.output || run.error) && (
        <div className={`run-exit-code ${run.exitCode === 0 ? 'success' : 'error'}`}>
          Exit code: {run.exitCode}
        </div>
      )}
    </div>
  );
}

function TokenView({
  json,
  text,
  error,
  format,
}: {
  json: string | null;
  text: string | null;
  error: string | null;
  format: 'text' | 'json';
}) {
  if (error) return <div className="diagnostic-error">{error}</div>;

  if (format === 'text') {
    return <div className="diagnostic-content">{text}</div>;
  }

  if (!json) return <div className="diagnostic-empty">No data</div>;

  let tokens: any[];
  try {
    tokens = JSON.parse(json);
  } catch {
    return <div className="diagnostic-error">Failed to parse JSON</div>;
  }

  return (
    <div className="token-list">
      {tokens.map((token, i) => {
        const isObj = typeof token === 'object' && token !== null;
        const type = isObj ? Object.keys(token)[0] : token;
        const value = isObj ? JSON.stringify(Object.values(token)[0]) : null;

        return (
          <div key={i} className="token-item">
            <span className="token-index">{i}</span>
            <span className="token-type">{type}</span>
            {value !== null && <span className="token-value">{value}</span>}
          </div>
        );
      })}
    </div>
  );
}

function JsonTextView({
  json,
  text,
  error,
  format,
}: {
  json: string | null;
  text: string | null;
  error: string | null;
  format: 'text' | 'json';
}) {
  if (error) return <div className="diagnostic-error">{error}</div>;

  if (format === 'text') {
    return <div className="diagnostic-content">{text}</div>;
  }

  if (!json) return <div className="diagnostic-empty">No data</div>;

  try {
    const parsed = JSON.parse(json);
    const formatted = JSON.stringify(parsed, null, 2);
    return <div className="diagnostic-content">{formatted}</div>;
  } catch {
    return <div className="diagnostic-error">Failed to parse JSON</div>;
  }
}

function IrView({
  json,
  text,
  error,
  format,
}: {
  json: string | null;
  text: string | null;
  error: string | null;
  format: 'text' | 'json';
}) {
  if (error) return <div className="diagnostic-error">{error}</div>;

  if (format === 'text') {
    if (!text) return <div className="diagnostic-empty">No data</div>;
    return <IrTextView text={text} />;
  }

  if (!json) return <div className="diagnostic-empty">No data</div>;

  try {
    const parsed = JSON.parse(json);
    const formatted = JSON.stringify(parsed, null, 2);
    return <div className="diagnostic-content">{formatted}</div>;
  } catch {
    return <div className="diagnostic-error">Failed to parse JSON</div>;
  }
}

function IrTextView({ text }: { text: string }) {
  const lines = text.split('\n');

  return (
    <div className="json-tree">
      {lines.map((line, i) => {
        if (line.startsWith('===')) {
          return (
            <div key={i} className="ir-section-header">
              {line}
            </div>
          );
        }
        if (line.startsWith('block')) {
          return (
            <div key={i} className="ir-block-header">
              {line}
            </div>
          );
        }
        if (line.match(/^\s+t\d+/)) {
          return <IrTermLine key={i} line={line} />;
        }
        if (line.startsWith('  fn') || line.startsWith('  c')) {
          return (
            <div key={i} className="ir-term" style={{ color: '#d4d4d4' }}>
              {line}
            </div>
          );
        }
        return (
          <div key={i} className="ir-term">
            {line}
          </div>
        );
      })}
    </div>
  );
}

function IrTermLine({ line }: { line: string }) {
  // Parse: "  t21 r21 = Constant(c0) [] ; x"
  const match = line.match(
    /^(\s+)(t\d+)\s+(r\d+)\s*=\s*(\S+(?:\(.*?\))?)\s*\[(.*?)\](.*)/,
  );
  if (!match) {
    return <div className="ir-term">{line}</div>;
  }

  const [, indent, termId, reg, op, inputs, rest] = match;
  // rest might contain "-> block1, block2 ; name"
  const blockMatch = rest.match(/\s*->\s*(.*?)(?:\s*;\s*(.*))?$/);
  const nameMatch = !blockMatch ? rest.match(/\s*;\s*(.*)$/) : null;

  return (
    <div className="ir-term">
      {indent}
      <span className="ir-term-id">{termId}</span>{' '}
      <span className="ir-term-reg">{reg}</span>
      {' = '}
      <span className="ir-term-op">{op}</span>
      {' '}
      <span className="ir-term-inputs">[{inputs}]</span>
      {blockMatch && (
        <>
          {' -> '}
          <span className="ir-term-blocks">{blockMatch[1]}</span>
          {blockMatch[2] && (
            <>
              {' ; '}
              <span className="ir-term-name">{blockMatch[2]}</span>
            </>
          )}
        </>
      )}
      {nameMatch && (
        <>
          {' ; '}
          <span className="ir-term-name">{nameMatch[1]}</span>
        </>
      )}
    </div>
  );
}
