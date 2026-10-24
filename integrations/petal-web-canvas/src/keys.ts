/** DOM key names to canonical petal-ui key names. Kept apart from input.ts,
 * which needs the generated wasm types, so it can be imported without a
 * built pkg. */

// Map DOM key names to canonical petal-ui key names (petal::keys::KEY_NAMES).
const KEY_MAP: Record<string, string> = {
  ArrowUp: "up",
  ArrowDown: "down",
  ArrowLeft: "left",
  ArrowRight: "right",
  Enter: "return",
  Escape: "escape",
  Backspace: "backspace",
  Tab: "tab",
  " ": "space",
  Shift: "shift",
  Control: "ctrl",
  Alt: "alt",
  Meta: "cmd",
  // Punctuation is named, not spelled: the key `-` is "minus". A shifted
  // character is the key it is typed on (`_` is still "minus"), as on the
  // SDL host, whose scancodes don't see Shift either.
  "-": "minus", _: "minus",
  "=": "equals", "+": "equals",
  ",": "comma", "<": "comma",
  ".": "period", ">": "period",
  "/": "slash", "?": "slash",
  "\\": "backslash", "|": "backslash",
  ";": "semicolon", ":": "semicolon",
  "'": "quote", '"': "quote",
  "`": "backquote", "~": "backquote",
  "[": "leftbracket", "{": "leftbracket",
  "]": "rightbracket", "}": "rightbracket",
  "!": "1", "@": "2", "#": "3", "$": "4", "%": "5",
  "^": "6", "&": "7", "*": "8", "(": "9", ")": "0",
};

// The keypad by physical key (`KeyboardEvent.code`): each key is the main-row
// key it prints, keypad Enter included, whatever Num Lock says — the same
// mapping as the SDL host. Keypad keys with no main-row twin (`+`, `*`) are
// not keys a script can name, and map to null.
const KEYPAD_MAP: Record<string, string | null> = {
  NumpadEnter: "return",
  NumpadSubtract: "minus",
  NumpadDecimal: "period",
  NumpadComma: "comma",
  NumpadDivide: "slash",
  NumpadEqual: "equals",
};

/** The canonical key name for a DOM key event (`e.key`, `e.code`), or null
 * when the key has none. */
export function mapKey(key: string, code = ""): string | null {
  if (code.startsWith("Numpad")) {
    const digit = /^Numpad([0-9])$/.exec(code);
    if (digit) return digit[1];
    return KEYPAD_MAP[code] ?? null;
  }
  if (KEY_MAP[key]) return KEY_MAP[key];
  // Single character (a letter or digit) → lowercase; a named key such as
  // "Home", "PageUp", "Delete" or "F5" lowercases to its canonical name too.
  return key.toLowerCase();
}
