# Petal Programming Language Specification (Lisp Dialect)

Petal is a functional programming language designed for creative coding. This dialect uses Lisp-style S-expression syntax while preserving Petal's core philosophy: dataflow-oriented programming, expression-based evaluation, built-in state management, and optional type annotations.

## Key Characteristics

- **Dataflow-first**: The `->` and `->>` threading macros create visual, pipeline-like programming patterns
- **Homoiconic**: Code is data; programs are represented as nested lists
- **Expression-oriented**: Everything returns a value, including control flow
- **Immutable by default**: Persistent data structures with functional updates
- **Built-in state system**: The `state` form creates retained data across invocations
- **Pattern matching**: First-class `match` expressions with destructuring
- **Multi-target**: Can run on an interpreter, GPU, or be transpiled

---

## Table of Contents

1. [Lexical Elements & Tokens](#1-lexical-elements--tokens)
2. [Data Types & Literals](#2-data-types--literals)
3. [Variables & Bindings](#3-variables--bindings)
4. [Expressions](#4-expressions)
5. [Functions](#5-functions)
6. [Control Flow](#6-control-flow)
7. [Pattern Matching](#7-pattern-matching)
8. [Dataflow Programming](#8-dataflow-programming)
9. [Structures & Types](#9-structures--types)
10. [State Management](#10-state-management)
11. [Built-in Functions & Standard Library](#11-built-in-functions--standard-library)
12. [Property Access](#12-property-access)
13. [Macros](#13-macros)
14. [Program Structure](#14-program-structure)
15. [Language Features Summary](#15-language-features-summary)
16. [Programming Patterns](#16-programming-patterns)

---

## 1. Lexical Elements & Tokens

### 1.1 Special Forms

```
defn     - Function definition
def      - Variable binding
let      - Local bindings
fn       - Anonymous function (lambda)
if       - Conditional
cond     - Multi-branch conditional
when     - Single-branch conditional
loop     - Loop with recur
for      - List comprehension / iteration
while    - While loop
match    - Pattern matching
state    - State variable declaration
do       - Sequential evaluation
quote    - Quote expression
```

### 1.2 Operators

Operators are functions and can be used in prefix notation:

| Category | Functions |
|----------|-----------|
| Arithmetic | `+`, `-`, `*`, `/`, `mod`, `**` (pow) |
| Comparison | `=`, `not=`, `<`, `>`, `<=`, `>=` |
| Logical | `and`, `or`, `not` |
| Dataflow | `->`, `->>`, `as->` (threading macros) |
| Collection | `get`, `assoc`, `update`, `conj`, `first`, `rest` |

### 1.3 Delimiters

```
( )     - Lists and function calls
[ ]     - Vectors (arrays)
{ }     - Maps (objects/hash-maps)
#{ }    - Sets
'       - Quote shorthand
`       - Quasiquote
,       - Unquote (inside quasiquote)
,@      - Unquote-splicing
:       - Keyword prefix
;       - Comment to end of line
```

### 1.4 Comments

```petal
; Single-line comment

;; Convention for section comments

#|
  Multi-line comment
  spanning lines
|#

;;; Documentation comment
;;; with details
```

---

## 2. Data Types & Literals

### 2.1 Primitive Types

**Integers**
```petal
42                  ; Decimal integer
0xFF                ; Hexadecimal
0b1010              ; Binary
-5                  ; Negative integer
```

**Floating Point**
```petal
3.14159             ; Float literal
1.23e-4             ; Scientific notation
-2.5                ; Negative float
```

**Strings**
```petal
"hello"             ; String literal
""                  ; Empty string
"Hello, ~{name}!"   ; String interpolation (~ prefix)

#"
  Multiline
  string
"#                  ; Multiline string (raw string)
```

**Booleans**
```petal
true                ; Boolean true
false               ; Boolean false
```

**Nil**
```petal
nil                 ; Nil/empty value (replaces null)
```

**Colors**
```petal
#c/FF0000           ; Red (RGB hex) - tagged literal
#c/00FF00FF         ; Green with alpha
#c/FFFFFF80         ; White, semi-transparent
```

**Keywords (Symbols)**
```petal
:symbol-name        ; Keyword literal
:error              ; Used for tagging
:fast               ; Enumeration-like values
```

**Symbols**
```petal
'my-symbol          ; Quoted symbol
'calculate-area     ; Symbols for metaprogramming
```

### 2.2 Collection Types

**Vectors (Arrays)**
```petal
[]                  ; Empty vector
[1 2 3]             ; Vector of integers
[1 "hello" true]    ; Mixed types
[[1 2] [3 4]]       ; Nested vectors
```

**Maps (Objects)**
```petal
{}                  ; Empty map
{:x 10 :y 20}       ; Map with keyword keys
{:name "Alice"
 :age 30
 :active true}      ; Multiline map

{"string-key" 42}   ; String keys also allowed
```

**Lists**
```petal
'()                 ; Empty list (quoted)
'(1 2 3)            ; Quoted list
(list 1 2 3)        ; List constructor
```

**Sets**
```petal
#{}                 ; Empty set
#{1 2 3}            ; Set of integers
#{:red :green :blue} ; Set of keywords
```

---

## 3. Variables & Bindings

### 3.1 Top-Level Definitions

```petal
(def x 42)
(def name "Alice")
(def flag true)

; With type annotations (optional)
(def ^:int count 100)
(def ^:float price 99.99)
(def ^:string message "Hello")
(def ^:bool active true)
```

### 3.2 Local Bindings

```petal
(let [x 42
      y 10
      z (+ x y)]
  (* z 2))

; Destructuring vectors
(let [[x y] [10 20]]
  (+ x y))

(let [[first & rest] [1 2 3 4 5]]
  {:first first :rest rest})

; Destructuring maps
(let [{:keys [name age]} {:name "Alice" :age 30}]
  (str name " is " age))

(let [{name :name age :age} {:name "Bob" :age 25}]
  (str name " is " age))
```

### 3.3 Mutation (When Needed)

```petal
; Using atoms for mutable state (explicit)
(def counter (atom 0))
(swap! counter inc)
(reset! counter 100)
@counter  ; Dereference: 100
```

---

## 4. Expressions

### 4.1 Literal Expressions

Values evaluate to themselves:
```petal
42
3.14
"hello"
true
nil
:symbol
[1 2 3]
{:a 1}
```

### 4.2 Function Application

All operations use prefix notation:
```petal
; Arithmetic
(+ a b)               ; Addition
(- a b)               ; Subtraction
(* a b)               ; Multiplication
(/ a b)               ; Division
(mod a b)             ; Modulo
(** a b)              ; Exponentiation

; Comparison
(= a b)               ; Equal
(not= a b)            ; Not equal
(< a b)               ; Less than
(<= a b)              ; Less than or equal
(> a b)               ; Greater than
(>= a b)              ; Greater than or equal

; Logical
(and a b)             ; Logical AND (short-circuit)
(or a b)              ; Logical OR (short-circuit)
(not flag)            ; Logical NOT
```

### 4.3 Nested Expressions

```petal
; Expressions can be arbitrarily nested
(+ (* 2 3) (/ 10 2))  ; => 11

; Complex calculations
(sqrt (+ (* dx dx) (* dy dy)))
```

### 4.4 do Blocks (Sequential Evaluation)

```petal
(do
  (print "Starting")
  (process-data)
  (print "Done")
  result)  ; Last expression is returned
```

---

## 5. Functions

### 5.1 Function Definition

```petal
; Basic function
(defn greet [name]
  (print (str "Hello, " name)))

; With multiple parameters
(defn add [a b]
  (+ a b))

; Single-expression function (implicit return)
(defn square [x]
  (* x x))

; With type annotations
(defn add ^:int [^:int a ^:int b]
  (+ a b))

; Multi-arity function
(defn greet
  ([] (greet "World"))
  ([name] (print (str "Hello, " name))))

; Variadic function
(defn log [& args]
  (send-effect :logs args))

; With docstring
(defn calculate-area
  "Calculates the area of a rectangle"
  [width height]
  (* width height))
```

### 5.2 Function Calls

```petal
; Basic calls
(print "hello")
(add 1 2)
(max x y)

; Nested calls
(add (square 3) (square 4))

; With keyword arguments (using maps)
(draw-line {:x1 0 :y1 0 :x2 100 :y2 100})
(create-window {:width 800 :height 600 :title "Game"})

; Method-style calls (using threading)
(-> player (move 10 5))
(-> text length)
```

### 5.3 Anonymous Functions (Lambdas)

```petal
; Full lambda syntax
(fn [x] (* x x))
(fn [a b] (+ a b))

; Short lambda syntax (reader macro)
#(* % %)              ; Single argument as %
#(+ %1 %2)            ; Multiple arguments as %1, %2, etc.

; Multi-line lambda
(fn [x y]
  (let [intermediate (+ (* x x) (* y y))]
    (sqrt intermediate)))

; With captured variables
(let [multiplier 10]
  (fn [x] (* x multiplier)))

; In higher-order functions
(->> numbers
     (filter #(= 0 (mod % 2)))
     (map #(* % 2)))
```

### 5.4 Closures & Capturing

Lambdas automatically capture variables from their enclosing scope:

```petal
(defn create-adder [n]
  (fn [x] (+ x n)))  ; Captures 'n'

(def add-five (create-adder 5))
(add-five 3)  ; => 8
```

---

## 6. Control Flow

### 6.1 Conditionals

```petal
; Basic if (two branches required)
(if (> x 5)
  "greater"
  "not greater")

; if with side effects
(if (> x 5)
  (do
    (print "x is greater than 5")
    :greater)
  (do
    (print "x is not greater than 5")
    :not-greater))

; when (single branch, returns nil if false)
(when (> x 5)
  (print "x is greater than 5")
  :greater)

; cond (multi-branch)
(cond
  (> x 5) "greater than 5"
  (< x 5) "less than 5"
  :else   "equals 5")

; Nested conditionals
(when (:authenticated user)
  (when (has-permission? user "admin")
    (show-admin-panel)))
```

### 6.2 For Comprehensions

```petal
; Basic iteration
(for [i (range 10)]
  (print i))

; With filter (when clause)
(for [i (range 100)
      :when (= 0 (mod i 5))]
  i)

; Nested iteration
(for [x (range 3)
      y (range 3)]
  [x y])

; With let bindings
(for [item items
      :let [doubled (* item 2)]
      :when (> doubled 10)]
  doubled)

; Iterate with index
(for [[idx value] (map-indexed vector items)]
  (str "Item at " idx ": " value))
```

### 6.3 Loop/Recur

```petal
; Basic loop
(loop [count 0]
  (when (< count 10)
    (print count)
    (recur (inc count))))

; Accumulating results
(loop [nums [1 2 3 4 5]
       sum 0]
  (if (empty? nums)
    sum
    (recur (rest nums) (+ sum (first nums)))))
```

### 6.4 While Loops

```petal
; While with state
(let [count (atom 0)]
  (while (< @count 10)
    (print @count)
    (swap! count inc)))

; Complex condition
(while (and searching (< attempts max-attempts))
  (let [result (try-search)]
    (when (not (nil? result))
      (set! searching false))
    (swap! attempts inc)))
```

### 6.5 Infinite Loop with Break

```petal
; Using loop with explicit exit
(loop []
  (let [input (get-user-input)]
    (if (= input "quit")
      :done
      (do
        (process-input input)
        (recur)))))
```

### 6.6 Early Return

```petal
; Pattern: use cond or nested if for early returns
(defn validate-input [input]
  (cond
    (nil? input)
    {:error "Input cannot be null"}

    (= 0 (length input))
    {:error "Input cannot be empty"}

    :else
    {:ok input}))
```

---

## 7. Pattern Matching

### 7.1 Basic Match

```petal
(match value
  0 "zero"
  1 "one"
  2 "two"
  _ "many")
```

### 7.2 Match with Guards

```petal
(match number
  (n :guard neg?) "negative"
  (n :guard #(> % 100)) "large"
  n (str "normal: " n))
```

### 7.3 Destructuring Patterns

**Vector destructuring:**
```petal
(match point
  [0 0] "origin"
  [x 0] (str "on x-axis at " x)
  [0 y] (str "on y-axis at " y)
  [x y] (str "at (" x ", " y ")"))

; Rest patterns
(match coordinates
  [] "empty"
  [x] "single"
  [x & rest] (str "first: " x ", rest: " (count rest)))
```

**Map destructuring:**
```petal
(match user
  {:name "admin" :role "administrator"}
  (grant-full-access)

  {:name name :role "user" :active true}
  (grant-user-access name)

  {:role "guest"}
  (grant-guest-access)

  _
  (deny-access "unknown user type"))
```

### 7.4 Enum Pattern Matching

```petal
(defenum Shape
  (Circle [radius :float])
  (Rectangle [width :float height :float])
  (Triangle [a :float b :float c :float]))

(defn calculate-area [shape]
  (match shape
    (Circle r)
    (* 3.14159 r r)

    (Rectangle w h)
    (* w h)

    (Triangle a b c)
    (let [s (/ (+ a b c) 2.0)]
      (sqrt (* s (- s a) (- s b) (- s c))))))
```

### 7.5 Nested Pattern Matching

```petal
(match event
  {:type "click" :position {:x x :y y}}
  (handle-click x y)

  {:type "key" :key key-code}
  (handle-key key-code)

  _
  nil)
```

---

## 8. Dataflow Programming

### 8.1 Threading Macros

The `->` (thread-first) and `->>` (thread-last) macros enable pipeline-style programming:

```petal
; Thread-first: inserts result as first argument
(-> player
    (move 10 5)
    (set-health 100)
    (add-item :sword))

; Thread-last: inserts result as last argument
(->> [1 2 3 4 5]
     (filter odd?)
     (map #(* % 2))
     (reduce +))  ; => 18

; Complex processing pipeline
(defn process-data [data]
  (->> data
       validate
       clean
       transform
       analyze
       save))
```

### 8.2 as-> Threading (Named Intermediate)

```petal
; When you need the value in different positions
(as-> {:x 10 :y 20} point
  (assoc point :z 30)
  (update point :x #(* % 2))
  (vals point)
  (reduce + 0 point))
```

### 8.3 Map Updates with Threading

```petal
; Simple update
(-> player
    (assoc :health 100))

; Multiple field updates
(-> game
    (assoc :score (+ (:score game) 100))
    (update :lives dec)
    (assoc :level 2))

; Nested updates
(-> user
    (update-in [:profile :name] (constantly "New Name"))
    (assoc-in [:profile :email] "new@example.com"))
```

### 8.4 The Dataflow Macro (Petal-specific)

For even more expressive dataflow, Petal provides a special `flow` macro:

```petal
(flow initial-state
  (assoc :score 0)
  handle-input
  update-physics
  (assoc :frame-count (inc (:frame-count initial-state))))
```

### 8.5 Conditional Threading

```petal
; cond-> threads when predicates are true
(cond-> player
  dead?        (assoc :state :respawning)
  low-health?  (apply-regen)
  has-powerup? (boost-stats))

; cond->> for thread-last style
(cond->> items
  filter?     (filter valid?)
  transform?  (map process)
  limit?      (take n))

; some-> stops on nil
(some-> user
        :profile
        :settings
        :theme)  ; Returns nil if any step is nil
```

---

## 9. Structures & Types

### 9.1 Struct Definition

```petal
(defstruct Point
  [x :float
   y :float])

(defstruct Player
  [name :string
   position Point
   health :int
   inventory [:string]])

; Generic struct
(defstruct Container [T]
  [items [T]
   capacity :int])
```

### 9.2 Struct Instantiation

```petal
(def p1 (Point {:x 0.0 :y 0.0}))
(def p2 (Point {:x 10.0 :y 20.0}))

; Using positional syntax
(def p3 (Point 10.0 20.0))

(def player (Player {:name "Alice"
                     :position p1
                     :health 100
                     :inventory []}))
```

### 9.3 Methods on Structs

```petal
; Define method using defmethod
(defmethod Point distance-to [self other]
  (let [dx (- (:x other) (:x self))
        dy (- (:y other) (:y self))]
    (sqrt (+ (* dx dx) (* dy dy)))))

(defmethod Rectangle area [self]
  (* (:width self) (:height self)))

; Usage
(def p1 (Point {:x 0.0 :y 0.0}))
(def p2 (Point {:x 3.0 :y 4.0}))
(distance-to p1 p2)  ; => 5.0

; Or with threading
(-> p1 (distance-to p2))
```

### 9.4 Enums

**Simple enum:**
```petal
(defenum Color
  Red
  Green
  Blue)

(def red Color/Red)
```

**Enum with data (algebraic data types):**
```petal
(defenum Shape
  (Circle [radius :float])
  (Rectangle [width :float height :float])
  (Triangle [a :float b :float c :float]))

(def circle (Shape/Circle {:radius 5.0}))
(def rect (Shape/Rectangle {:width 10.0 :height 20.0}))
```

**Generic enum:**
```petal
(defenum Result [T E]
  (Ok [value T])
  (Err [error E]))

(defenum Option [T]
  (Some [value T])
  None)
```

### 9.5 Type Annotations (Optional)

```petal
(def ^:int x 42)
(def ^:string name "Alice")
(def ^:float count 3.14)
(def ^:bool flag true)

(defn ^:int add [^:int a ^:int b]
  (+ a b))

; Collection types
(def ^[:int] numbers [1 2 3])
(def ^{:string :int} scores {"alice" 100 "bob" 95})
```

### 9.6 Protocols (Interfaces)

```petal
(defprotocol Drawable
  (draw [self canvas])
  (bounds [self]))

(extend-type Circle
  Drawable
  (draw [self canvas]
    (draw-circle canvas (:x self) (:y self) (:radius self)))
  (bounds [self]
    {:x (- (:x self) (:radius self))
     :y (- (:y self) (:radius self))
     :width (* 2 (:radius self))
     :height (* 2 (:radius self))}))
```

---

## 10. State Management

The `state` form creates retained data that persists across function calls, similar to React's useState hook but integrated into the language.

### 10.1 Basic State

```petal
(defn counter []
  (state count 0)  ; Retained across function calls

  (set! count (inc count))
  (when (> count 100)
    (set! count 0))

  count)
```

### 10.2 Complex State Structures

```petal
(defn particle-system []
  (state particles [])
  (state emitter {:position [0.0 0.0]
                  :rate 10.0
                  :timer 0.0})

  (let [dt (get-delta-time)]
    ; Update timer
    (update! emitter :timer #(+ % dt))

    ; Emit new particles
    (when (> (:timer emitter) (/ 1.0 (:rate emitter)))
      (let [new-particle (create-particle (:position emitter))]
        (update! particles conj new-particle)
        (assoc! emitter :timer 0.0)))

    ; Update and filter particles
    (set! particles
      (->> particles
           (map #(update-particle % dt))
           (filter #(> (:life %) 0.0)))))

  particles)
```

### 10.3 State in Control Flow

**State in loops:**
```petal
(defn animated-grid [width height]
  (for [y (range height)]
    (for [x (range width)]
      (do
        (state cell-phase (random 0.0 6.28))   ; Each cell has own state
        (state cell-amplitude (random 0.5 1.5))

        (update! cell-phase #(+ % (* (get-delta-time) 2.0)))
        (* (sin cell-phase) cell-amplitude)))))
```

**State in conditionals:**
```petal
(defn adaptive-behavior [mode]
  (if (= mode :fast)
    (do
      (state fast-counter 0)
      (update! fast-counter #(+ % 2))
      fast-counter)
    (do
      (state slow-counter 0)
      (update! slow-counter inc)
      slow-counter)))
```

### 10.4 Animation with State

```petal
(defn smooth-transition [target-value]
  (state current-value target-value)
  (state velocity 0.0)

  (let [spring-force 0.1
        damping 0.8
        dt (get-delta-time)
        force (* (- target-value current-value) spring-force)]

    (update! velocity #(* (+ % force) damping))
    (update! current-value #(+ % (* velocity dt))))

  current-value)
```

### 10.5 State Machines

```petal
(defn complex-animation []
  (state phase :idle)
  (state time-in-phase 0.0)
  (state animation-data {:position [0.0 0.0]
                         :rotation 0.0
                         :scale 1.0})

  (let [dt (get-delta-time)]
    (update! time-in-phase #(+ % dt))

    (match phase
      :idle
      (do
        (update! animation-data
          assoc :scale (+ 1.0 (* (sin (* time-in-phase 2.0)) 0.05)))
        (when (> time-in-phase 3.0)
          (set! phase :moving)
          (set! time-in-phase 0.0)))

      :moving
      (let [progress (/ time-in-phase 2.0)]
        (update! animation-data
          assoc-in [:position 0] (* (easing-out-cubic progress) 200.0))
        (when (>= progress 1.0)
          (set! phase :idle)
          (set! time-in-phase 0.0)))))

  animation-data)
```

---

## 11. Built-in Functions & Standard Library

### 11.1 I/O Functions

```petal
(print value)           ; Print value to console
(println value)         ; Print with newline
(pr value)              ; Print readable form
(read)                  ; Read from input
(slurp path)            ; Read file contents
(spit path content)     ; Write file contents
```

### 11.2 Collection Operations

```petal
; Sequence operations
(filter pred coll)      ; Filter elements
(map f coll)            ; Transform elements
(reduce f init coll)    ; Reduce to single value
(fold f init coll)      ; Parallel-friendly reduce

; Aggregation
(sum coll)              ; Sum array elements
(average coll)          ; Average of elements

; Ordering
(sort coll)             ; Sort array
(sort-by key-fn coll)   ; Sort by key function
(reverse coll)          ; Reverse array

; Taking/dropping
(take n coll)           ; Take first n elements
(drop n coll)           ; Drop first n elements
(take-while pred coll)  ; Take while predicate true
(drop-while pred coll)  ; Drop while predicate true

; Access
(first coll)            ; First element
(rest coll)             ; All but first
(last coll)             ; Last element
(butlast coll)          ; All but last
(nth coll n)            ; Element at index
(get coll key)          ; Get by key (maps) or index

; Indexing
(map-indexed f coll)    ; Map with index

; Map operations
(keys m)                ; Get map keys
(vals m)                ; Get map values
(entries m)             ; Get [key value] pairs

; Building
(conj coll item)        ; Add to collection
(assoc m key val)       ; Associate key-value
(dissoc m key)          ; Remove key
(update m key f)        ; Update value at key
(merge m1 m2)           ; Merge maps
```

### 11.3 Math Functions

```petal
(sqrt x)                ; Square root
(sin x)                 ; Sine (radians)
(cos x)                 ; Cosine
(tan x)                 ; Tangent
(** base exp)           ; Power function (or pow)
(abs x)                 ; Absolute value
(max a b)               ; Maximum
(min a b)               ; Minimum
(floor x)               ; Floor
(ceil x)                ; Ceiling
(round x)               ; Round to nearest
(lerp a b t)            ; Linear interpolation
(clamp x min max)       ; Clamp value
(random min max)        ; Random number in range
(rand)                  ; Random 0.0-1.0
```

### 11.4 String Functions

```petal
(count s)               ; String length
(str a b c)             ; Concatenate to string
(trim s)                ; Trim whitespace
(lower-case s)          ; Convert to lowercase
(upper-case s)          ; Convert to uppercase
(split s sep)           ; Split string
(replace s a b)         ; Replace substring
(includes? s sub)       ; Check if contains
(starts-with? s prefix) ; Check prefix
(ends-with? s suffix)   ; Check suffix
(join sep coll)         ; Join with separator
(subs s start end)      ; Substring
```

### 11.5 Predicates

```petal
(nil? x)                ; Is nil?
(some? x)               ; Is not nil?
(empty? coll)           ; Is empty?
(seq coll)              ; Nil if empty, else seq
(pos? n)                ; Is positive?
(neg? n)                ; Is negative?
(zero? n)               ; Is zero?
(even? n)               ; Is even?
(odd? n)                ; Is odd?
(fn? x)                 ; Is function?
(keyword? x)            ; Is keyword?
(string? x)             ; Is string?
(number? x)             ; Is number?
(vector? x)             ; Is vector?
(map? x)                ; Is map?
```

---

## 12. Property Access

### 12.1 Keyword Access

Keywords can be used as functions to access map values:
```petal
(:name person)          ; Get :name from person
(:length text)          ; Get :length property
(:city (:address user)) ; Chained access

; Same as:
(get person :name)
```

### 12.2 Bracket/Index Access

```petal
(get numbers 0)         ; First element
(nth numbers 0)         ; Also first element
(get numbers -1)        ; Last element (negative indexing)
(get-in matrix [row col]) ; Multi-dimensional
```

### 12.3 Safe Access (Optional Chaining)

```petal
; Using some-> for safe navigation
(some-> user :address :city)  ; Returns nil if any part is nil

; With default
(or (some-> user :address :city) "Unknown")

; Using get-in with default
(get-in user [:address :city] "Unknown")
```

---

## 13. Macros

Petal supports hygienic macros for metaprogramming.

### 13.1 Macro Definition

```petal
(defmacro unless [test & body]
  `(if (not ~test)
     (do ~@body)))

; Usage
(unless (empty? items)
  (process items)
  (save items))
```

### 13.2 Threading Macro Implementation

```petal
; Example: how -> might be implemented
(defmacro -> [x & forms]
  (loop [x x
         forms forms]
    (if (empty? forms)
      x
      (let [form (first forms)
            threaded (if (seq? form)
                       `(~(first form) ~x ~@(rest form))
                       `(~form ~x))]
        (recur threaded (rest forms))))))
```

### 13.3 Custom Control Flow

```petal
(defmacro with-timing [label & body]
  `(let [start# (now)]
     (let [result# (do ~@body)]
       (println ~label "took" (- (now) start#) "ms")
       result#)))

; Usage
(with-timing "Processing"
  (heavy-computation data))
```

---

## 14. Program Structure

### 14.1 Namespaces

```petal
(ns my-game.core
  (:require [petal.math :as m]
            [petal.graphics :refer [draw-circle draw-rect]]
            [my-game.physics :as physics]))

(defn main []
  (let [angle (m/random 0 (* 2 m/PI))]
    (draw-circle 100 100 50)))
```

### 14.2 File Structure

```
src/
  my-game/
    core.petal       ; Main entry point
    physics.petal    ; Physics module
    graphics.petal   ; Graphics module
    utils.petal      ; Utility functions
```

### 14.3 Entry Point

```petal
; main.petal
(ns main
  (:require [my-app.core :as app]))

(defn -main [& args]
  (app/start args))
```

---

## 15. Language Features Summary

| Feature | Status | Notes |
|---------|--------|-------|
| Variables & Bindings | ✓ | def, let with destructuring |
| Functions | ✓ | defn, fn, multi-arity |
| Control Flow | ✓ | if, cond, when, loop/recur, for |
| Pattern Matching | ✓ | match with guards and destructuring |
| Data Types | ✓ | Primitives, vectors, maps, sets, structs, enums |
| Type Annotations | ✓ | Optional metadata-style annotations |
| Lambdas/Closures | ✓ | First-class functions with capture |
| Threading Macros | ✓ | ->, ->>, as->, cond->, some-> |
| State Management | ✓ | state form for retained data |
| String Interpolation | ✓ | ~{variable} syntax |
| Generics | ✓ | Generic structs and enums |
| Protocols | ✓ | Interface definitions |
| Macros | ✓ | Hygienic macro system |
| Homoiconicity | ✓ | Code as data |
| Persistent Data | ✓ | Immutable by default |

---

## 16. Programming Patterns

### 16.1 Functional Programming

```petal
; Function composition
(defn compose [f g]
  (fn [x] (f (g x))))

(def inc-then-double (compose #(* % 2) inc))
(inc-then-double 3)  ; => 8

; Higher-order functions
(defn my-map [f coll]
  (for [item coll]
    (f item)))

; Recursive functions with TCO
(defn factorial [n]
  (loop [n n
         acc 1]
    (if (<= n 1)
      acc
      (recur (dec n) (* acc n)))))

; Or with match
(defn factorial [n]
  (match n
    0 1
    1 1
    n (* n (factorial (dec n)))))
```

### 16.2 Dataflow Pipelines

```petal
(->> raw-data
     validate
     (filter pos?)
     (map #(* % 2))
     (reduce +))

; With named steps for clarity
(defn process-orders [orders]
  (->> orders
       (filter :active)
       (map calculate-total)
       (group-by :region)
       (map-vals #(reduce + (map :total %)))))
```

### 16.3 State Machines

```petal
(defn state-machine []
  (state current-state :initial)

  (set! current-state
    (match current-state
      :initial :running
      :running :done
      :done    :initial))

  current-state)
```

### 16.4 Game Loop Pattern

```petal
(defn game-update []
  (state game (initialize-game))
  (state time 0.0)

  (update! time #(+ % (get-delta-time)))

  (-> game
      handle-input
      (update-physics time)
      render))
```

### 16.5 Error Handling

```petal
(defenum Result [T E]
  (Ok [value T])
  (Err [error E]))

(defn safe-divide [a b]
  (if (= b 0)
    (Result/Err "Division by zero")
    (Result/Ok (/ a b))))

; Usage with pattern matching
(match (safe-divide 10 2)
  (Ok result) (println "Result:" result)
  (Err msg)   (println "Error:" msg))

; Monadic chaining
(defn divide-chain [a b c]
  (-> (safe-divide a b)
      (bind #(safe-divide % c))))
```

### 16.6 Builder Pattern with Threading

```petal
(defn create-config []
  (-> {}
      (assoc :debug false)
      (assoc :log-level "info")
      (assoc :max-connections 100)))

; With conditional additions
(defn create-config [options]
  (cond-> {:debug false}
    (:verbose options) (assoc :log-level "debug")
    (:limit options)   (assoc :max-connections (:limit options))))
```

### 16.7 Transducers for Efficient Pipelines

```petal
; Composable transformations without intermediate collections
(def xf
  (comp
    (filter pos?)
    (map #(* % 2))
    (take 10)))

(transduce xf + 0 data)  ; Apply and reduce in one pass

(into [] xf data)        ; Apply and collect
```

---

## Appendix A: Reserved Words

The following are special forms and cannot be used as identifiers:

```
def, defn, defmacro, defstruct, defenum, defprotocol, defmethod,
let, fn, if, cond, when, unless, match, loop, recur, for, while,
do, quote, state, ns, require, import, true, false, nil
```

## Appendix B: File Extension

Petal Lisp source files use the `.petal` extension (same as standard Petal).

## Appendix C: Encoding

Petal source files are expected to be encoded in UTF-8.

## Appendix D: Comparison with Original Petal Syntax

| Original Petal | Lisp Petal |
|----------------|------------|
| `let x = 42` | `(def x 42)` |
| `fn add(a, b) { a + b }` | `(defn add [a b] (+ a b))` |
| `if x > 5 { ... }` | `(if (> x 5) ...)` |
| `data @ filter(f) @ map(g)` | `(->> data (filter f) (map g))` |
| `player @ { health: 100 }` | `(-> player (assoc :health 100))` |
| `match x { 0 -> "zero" }` | `(match x 0 "zero")` |
| `state count = 0` | `(state count 0)` |
| `[1, 2, 3]` | `[1 2 3]` |
| `{x: 10, y: 20}` | `{:x 10 :y 20}` |
| `:symbol` | `:symbol` (same!) |
| `fn(x) => x * 2` | `#(* % 2)` or `(fn [x] (* x 2))` |
