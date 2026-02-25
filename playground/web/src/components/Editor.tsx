import { useRef, useCallback } from 'react';

interface Example {
  filename: string;
  name: string;
}

interface EditorProps {
  code: string;
  onChange: (code: string) => void;
  examples: Example[];
  onLoadExample: (filename: string) => void;
}

export function Editor({ code, onChange, examples, onLoadExample }: EditorProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const handleKeyDown = useCallback((e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Tab') {
      e.preventDefault();
      const ta = e.currentTarget;
      const start = ta.selectionStart;
      const end = ta.selectionEnd;
      const newValue = code.substring(0, start) + '    ' + code.substring(end);
      onChange(newValue);
      // Restore cursor position after React re-render
      requestAnimationFrame(() => {
        ta.selectionStart = ta.selectionEnd = start + 4;
      });
    }
  }, [code, onChange]);

  return (
    <div className="editor-panel">
      <div className="editor-header">
        <h2>Source Code</h2>
        {examples.length > 0 && (
          <select
            className="example-picker"
            value=""
            onChange={(e) => {
              if (e.target.value) onLoadExample(e.target.value);
            }}
          >
            <option value="">Load example...</option>
            {examples.map((ex) => (
              <option key={ex.filename} value={ex.filename}>
                {ex.name}
              </option>
            ))}
          </select>
        )}
      </div>
      <textarea
        ref={textareaRef}
        className="editor-textarea"
        value={code}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder="Enter Petal code here..."
        spellCheck={false}
        autoComplete="off"
        autoCorrect="off"
        autoCapitalize="off"
      />
    </div>
  );
}
