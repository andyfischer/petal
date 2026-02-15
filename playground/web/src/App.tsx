import { useState, useCallback, useRef, useEffect } from 'react';
import { webFetch } from '@facetlayer/prism-framework-ui';
import { Editor } from './components/Editor';
import { DiagnosticPanel } from './components/DiagnosticPanel';
import './App.css';

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

const DEFAULT_CODE = `// Welcome to the Petal Playground!
// Edit code here and see the compiler internals on the right.

fn greet(name) {
    "Hello, " ++ name ++ "!"
}

let message = greet("world")
print(message)

for i in range(1, 5) {
    print(str(i) ++ "...")
}
`;

export function App() {
  const [code, setCode] = useState(DEFAULT_CODE);
  const [analysis, setAnalysis] = useState<AnalysisResult | null>(null);
  const [textResult, setTextResult] = useState<TextResult | null>(null);
  const [analyzing, setAnalyzing] = useState(false);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const analyze = useCallback(async (source: string) => {
    setAnalyzing(true);
    try {
      const [jsonResult, textRes] = await Promise.all([
        webFetch('POST /analyze', { params: { code: source } }),
        webFetch('POST /analyze-text', { params: { code: source } }),
      ]);
      setAnalysis(jsonResult as AnalysisResult);
      setTextResult(textRes as TextResult);
    } catch {
      setAnalysis(null);
      setTextResult(null);
    } finally {
      setAnalyzing(false);
    }
  }, []);

  const handleCodeChange = useCallback((newCode: string) => {
    setCode(newCode);
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => analyze(newCode), 400);
  }, [analyze]);

  // Initial analysis
  useEffect(() => {
    analyze(code);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <div className="app">
      <header className="app-header">
        <h1>Petal</h1>
        <span className="subtitle">Playground</span>
        {analyzing && <span className="analyzing-indicator">Analyzing...</span>}
      </header>
      <div className="app-body">
        <Editor code={code} onChange={handleCodeChange} />
        <DiagnosticPanel analysis={analysis} textResult={textResult} />
      </div>
    </div>
  );
}
